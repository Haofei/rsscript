use std::collections::HashMap;
use std::future::Future;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Mutex, Once, OnceLock};
use std::time::{Duration, Instant};

use sqlx::any::AnyPoolOptions;
use sqlx::{AnyPool, Row};
use tokio::runtime::{Builder, Runtime};

static INSTALL_DRIVERS: Once = Once::new();

pub const DEFAULT_MAX_CONNECTIONS: u32 = 8;
pub const DEFAULT_MAX_CACHED_POOLS: usize = 32;
pub const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 5_000;
pub const DEFAULT_QUERY_TIMEOUT_MS: u64 = 30_000;
pub const DEFAULT_POOL_IDLE_TIMEOUT_MS: u64 = 300_000;
pub const DEFAULT_MAX_RESULT_ROWS: usize = 10_000;
pub const DEFAULT_MAX_RESULT_BYTES: usize = 16 * 1024 * 1024;

const MAX_CACHED_POOLS_ENV: &str = "RSS_SQLX_MAX_CACHED_POOLS";
const CONNECT_TIMEOUT_MS_ENV: &str = "RSS_SQLX_CONNECT_TIMEOUT_MS";
const QUERY_TIMEOUT_MS_ENV: &str = "RSS_SQLX_QUERY_TIMEOUT_MS";
const POOL_IDLE_TIMEOUT_MS_ENV: &str = "RSS_SQLX_POOL_IDLE_TIMEOUT_MS";
const MAX_RESULT_ROWS_ENV: &str = "RSS_SQLX_MAX_RESULT_ROWS";
const MAX_RESULT_BYTES_ENV: &str = "RSS_SQLX_MAX_RESULT_BYTES";
const RUNTIME_QUEUE_CAPACITY: usize = 64;

#[derive(Clone, Copy)]
struct QueryLimits {
    max_rows: usize,
    max_bytes: usize,
}

#[derive(Clone, Copy)]
struct SqlxConfig {
    max_cached_pools: usize,
    connect_timeout: Duration,
    query_timeout: Duration,
    pool_idle_timeout: Duration,
}

type PoolKey = String;
type RuntimeJob = Box<dyn FnOnce(&Runtime) + Send + 'static>;

struct PoolEntry {
    pool: AnyPool,
    last_used: Instant,
    sequence: u64,
}

#[derive(Default)]
struct PoolRegistry {
    entries: HashMap<PoolKey, PoolEntry>,
    clock: u64,
}

fn configured_limit(name: &str, default: usize) -> Result<usize, String> {
    let Some(value) = std::env::var_os(name) else {
        return Ok(default);
    };
    let value = value
        .to_str()
        .ok_or_else(|| format!("{name} must be valid UTF-8"))?;
    value
        .parse::<usize>()
        .ok()
        .filter(|limit| *limit > 0)
        .ok_or_else(|| format!("{name} must be a positive integer"))
}

fn configured_duration(name: &str, default_ms: u64) -> Result<Duration, String> {
    let timeout_ms = configured_limit(name, default_ms as usize)?;
    Ok(Duration::from_millis(timeout_ms as u64))
}

fn configured_sqlx() -> Result<SqlxConfig, String> {
    Ok(SqlxConfig {
        max_cached_pools: configured_limit(MAX_CACHED_POOLS_ENV, DEFAULT_MAX_CACHED_POOLS)?,
        connect_timeout: configured_duration(CONNECT_TIMEOUT_MS_ENV, DEFAULT_CONNECT_TIMEOUT_MS)?,
        query_timeout: configured_duration(QUERY_TIMEOUT_MS_ENV, DEFAULT_QUERY_TIMEOUT_MS)?,
        pool_idle_timeout: configured_duration(
            POOL_IDLE_TIMEOUT_MS_ENV,
            DEFAULT_POOL_IDLE_TIMEOUT_MS,
        )?,
    })
}

fn configured_query_limits() -> Result<QueryLimits, String> {
    Ok(QueryLimits {
        max_rows: configured_limit(MAX_RESULT_ROWS_ENV, DEFAULT_MAX_RESULT_ROWS)?,
        max_bytes: configured_limit(MAX_RESULT_BYTES_ENV, DEFAULT_MAX_RESULT_BYTES)?,
    })
}

fn pool_key(url: &str) -> PoolKey {
    normalize_database_url(url)
}

fn normalize_database_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_owned();
    };
    let scheme = match scheme.to_ascii_lowercase().as_str() {
        "postgresql" => "postgres".to_owned(),
        other => other.to_owned(),
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, suffix) = rest.split_at(authority_end);
    let (userinfo, host_port) = authority
        .rsplit_once('@')
        .map_or(("", authority), |(userinfo, host)| (userinfo, host));
    let host_port = normalize_host_port(&scheme, host_port);
    let authority = if userinfo.is_empty() {
        host_port
    } else {
        format!("{userinfo}@{host_port}")
    };

    let (before_fragment, fragment) = suffix
        .split_once('#')
        .map_or((suffix, None), |(value, fragment)| (value, Some(fragment)));
    let (path, query) = before_fragment
        .split_once('?')
        .map_or((before_fragment, None), |(path, query)| (path, Some(query)));
    let query = query.map(|query| {
        let mut fields = query.split('&').collect::<Vec<_>>();
        fields.sort_unstable();
        fields.join("&")
    });

    let mut normalized = format!("{scheme}://{authority}{path}");
    if let Some(query) = query {
        normalized.push('?');
        normalized.push_str(&query);
    }
    if let Some(fragment) = fragment {
        normalized.push('#');
        normalized.push_str(fragment);
    }
    normalized
}

fn normalize_host_port(scheme: &str, host_port: &str) -> String {
    let default_port = match scheme {
        "postgres" => Some("5432"),
        "mysql" => Some("3306"),
        _ => None,
    };
    if let Some(close) = host_port.strip_prefix('[').and_then(|host| host.find(']')) {
        let bracket_end = close + 2;
        let host = host_port[..bracket_end].to_ascii_lowercase();
        let port = host_port[bracket_end..].strip_prefix(':');
        return match port {
            Some(port) if Some(port) == default_port => host,
            _ => format!("{host}{}", &host_port[bracket_end..]),
        };
    }
    match host_port.rsplit_once(':') {
        Some((host, port)) if port.bytes().all(|byte| byte.is_ascii_digit()) => {
            let host = host.to_ascii_lowercase();
            if Some(port) == default_port {
                host
            } else {
                format!("{host}:{port}")
            }
        }
        _ => host_port.to_ascii_lowercase(),
    }
}

fn url_fingerprint(url: &str) -> String {
    let mut hasher = DefaultHasher::new();
    pool_key(url).hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn redact_error(url: &str, error: impl std::fmt::Display) -> String {
    let replacement = format!("<database-url:{}>", url_fingerprint(url));
    let mut redacted = error.to_string();
    for candidate in database_url_secrets(url) {
        redacted = redacted.replace(&candidate, "<redacted>");
    }
    redacted = redacted.replace(url, &replacement);
    redacted.replace(&normalize_database_url(url), &replacement)
}

fn database_url_secrets(url: &str) -> Vec<String> {
    let Some((_, rest)) = url.split_once("://") else {
        return vec![url.to_owned()];
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, suffix) = rest.split_at(authority_end);
    let mut secrets = Vec::new();
    if let Some((userinfo, _)) = authority.rsplit_once('@')
        && let Some((_, password)) = userinfo.split_once(':')
        && !password.is_empty()
    {
        secrets.push(password.to_owned());
    }
    if let Some((query, _)) = suffix
        .split_once('?')
        .map(|(_, query)| query.split_once('#').unwrap_or((query, "")))
    {
        secrets.extend(query.split('&').filter_map(|field| {
            let (_, value) = field.split_once('=')?;
            (!value.is_empty()).then(|| value.to_owned())
        }));
    }
    secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    secrets.dedup();
    secrets
}

fn account_value(
    value: &str,
    row_count: usize,
    byte_count: &mut usize,
    limits: QueryLimits,
) -> Result<(), String> {
    if row_count >= limits.max_rows {
        return Err(format!(
            "query result exceeds row limit of {}",
            limits.max_rows
        ));
    }
    *byte_count = byte_count
        .checked_add(value.len())
        .filter(|bytes| *bytes <= limits.max_bytes)
        .ok_or_else(|| format!("query result exceeds byte limit of {}", limits.max_bytes))?;
    Ok(())
}

/// Shared Tokio runtime that drives SQLx futures outside a caller's runtime.
fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime for sqlx pool")
    })
}

fn runtime_jobs() -> &'static SyncSender<RuntimeJob> {
    static JOBS: OnceLock<SyncSender<RuntimeJob>> = OnceLock::new();
    JOBS.get_or_init(|| {
        let (sender, receiver) = mpsc::sync_channel::<RuntimeJob>(RUNTIME_QUEUE_CAPACITY);
        std::thread::Builder::new()
            .name("rss-sqlx-runtime".to_owned())
            .spawn(move || {
                while let Ok(job) = receiver.recv() {
                    job(runtime());
                }
            })
            .expect("SQLx runtime worker");
        sender
    })
}

fn submit_runtime_job(sender: &SyncSender<RuntimeJob>, job: RuntimeJob) -> Result<(), String> {
    match sender.try_send(job) {
        Ok(()) => Ok(()),
        Err(mpsc::TrySendError::Full(_)) => Err(format!(
            "SQLx runtime queue is full (capacity {RUNTIME_QUEUE_CAPACITY})"
        )),
        Err(mpsc::TrySendError::Disconnected(_)) => Err("SQLx runtime worker stopped".to_string()),
    }
}

fn run_on_runtime<F, T>(future: F) -> Result<T, String>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let (sender, receiver) = mpsc::sync_channel(1);
    submit_runtime_job(
        runtime_jobs(),
        Box::new(move |runtime| {
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runtime.block_on(future)))
                    .map_err(|_| "SQLx runtime worker panicked".to_string());
            let _ = sender.send(result);
        }),
    )?;
    receiver
        .recv()
        .map_err(|_| "SQLx runtime worker stopped".to_string())?
}

fn run_with_deadline<F, T>(
    url: &str,
    operation: &'static str,
    timeout: Duration,
    future: F,
) -> Result<T, String>
where
    F: Future<Output = Result<T, String>> + Send + 'static,
    T: Send + 'static,
{
    let result = run_on_runtime(async move { tokio::time::timeout(timeout, future).await })?;
    match result {
        Ok(result) => result.map_err(|error| {
            format!(
                "SQLx {operation} for <database-url:{}> failed: {}",
                url_fingerprint(url),
                redact_error(url, error)
            )
        }),
        Err(_) => Err(format!(
            "SQLx {operation} for <database-url:{}> timed out after {}ms",
            url_fingerprint(url),
            timeout.as_millis()
        )),
    }
}

fn pools() -> &'static Mutex<PoolRegistry> {
    static POOLS: OnceLock<Mutex<PoolRegistry>> = OnceLock::new();
    POOLS.get_or_init(|| Mutex::new(PoolRegistry::default()))
}

fn close_pools(mut removed: Vec<(PoolKey, AnyPool)>) -> Result<(), String> {
    removed.sort_by(|(left, _), (right, _)| left.cmp(right));
    run_on_runtime(async move {
        for (_, pool) in removed {
            pool.close().await;
        }
    })
}

fn pool_for_with_config(
    url: &str,
    max_cached_pools: usize,
    idle_timeout: Duration,
    connect_timeout: Duration,
) -> Result<AnyPool, String> {
    if max_cached_pools == 0 {
        return Err("maximum cached SQLx pools must be positive".to_string());
    }

    INSTALL_DRIVERS.call_once(sqlx::any::install_default_drivers);
    let key = pool_key(url);
    let now = Instant::now();
    let mut registry = pools().lock().map_err(|error| error.to_string())?;
    registry.clock = registry.clock.wrapping_add(1);
    let sequence = registry.clock;
    registry
        .entries
        .retain(|_, entry| now.duration_since(entry.last_used) < idle_timeout);
    if let Some(entry) = registry.entries.get_mut(&key) {
        entry.last_used = now;
        entry.sequence = sequence;
        return Ok(entry.pool.clone());
    }

    while registry.entries.len() >= max_cached_pools {
        let lru_key = registry
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.sequence)
            .map(|(key, _)| key.clone())
            .expect("a full positive-capacity cache has an entry");
        registry.entries.remove(&lru_key);
    }

    // Creating the lazy pool while holding the registry lock prevents duplicate
    // entries when native calls arrive concurrently.
    let pool = {
        let _enter = runtime().enter();
        AnyPoolOptions::new()
            .max_connections(DEFAULT_MAX_CONNECTIONS)
            .acquire_timeout(connect_timeout)
            .idle_timeout(Some(idle_timeout))
            .connect_lazy(url)
            .map_err(|error| {
                format!(
                    "SQLx pool creation for <database-url:{}> failed: {}",
                    url_fingerprint(url),
                    redact_error(url, error)
                )
            })?
    };
    registry.entries.insert(
        key,
        PoolEntry {
            pool: pool.clone(),
            last_used: now,
            sequence,
        },
    );
    Ok(pool)
}

fn pool_for(url: &str, config: SqlxConfig) -> Result<AnyPool, String> {
    pool_for_with_config(
        url,
        config.max_cached_pools,
        config.pool_idle_timeout,
        config.connect_timeout,
    )
}

/// Remove and close the cached pool for one URL. In-flight users may delay
/// completion until they return their connections.
pub fn close(url: &str) -> Result<(), String> {
    let key = pool_key(url);
    let removed = pools()
        .lock()
        .map_err(|error| error.to_string())?
        .entries
        .remove(&key)
        .map(|entry| (key, entry.pool));
    if let Some(pool) = removed {
        close_pools(vec![pool])?;
    }
    Ok(())
}

/// Remove and close every cached pool in deterministic fingerprint order.
pub fn close_all() -> Result<(), String> {
    let removed = {
        let mut registry = pools().lock().map_err(|error| error.to_string())?;
        registry
            .entries
            .drain()
            .map(|(key, entry)| (key, entry.pool))
            .collect()
    };
    close_pools(removed)
}

/// Run one or more SQL statements that produce no rows (DDL, `INSERT`, ...).
pub fn execute(url: &str, sql: &str) -> Result<(), String> {
    let config = configured_sqlx()?;
    let pool = pool_for(url, config)?;
    let sql = sql.to_owned();
    run_with_deadline(url, "query", config.query_timeout, async move {
        sqlx::raw_sql(&sql)
            .execute(&pool)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    })
}

/// Run one SQL statement with positional string bind parameters.
pub fn execute_params(url: &str, sql: &str, params: &[String]) -> Result<(), String> {
    let config = configured_sqlx()?;
    let pool = pool_for(url, config)?;
    let sql = sql.to_owned();
    let params = params.to_vec();
    run_with_deadline(url, "query", config.query_timeout, async move {
        let mut query = sqlx::query(&sql);
        for param in &params {
            query = query.bind(param);
        }
        query
            .execute(&pool)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    })
}

fn query_strings_with_limits(
    url: &str,
    sql: &str,
    params: &[String],
    limits: QueryLimits,
) -> Result<Vec<String>, String> {
    let config = configured_sqlx()?;
    let pool = pool_for(url, config)?;
    let sql = sql.to_owned();
    let params = params.to_vec();
    run_with_deadline(url, "query", config.query_timeout, async move {
        let mut query = sqlx::query(&sql);
        for param in &params {
            query = query.bind(param);
        }
        let mut rows = query.fetch(&pool);
        let mut values = Vec::new();
        let mut bytes = 0;
        while let Some(row) = std::future::poll_fn(|context| rows.as_mut().poll_next(context)).await
        {
            let row = row.map_err(|error| error.to_string())?;
            let value: &str = row.try_get(0).map_err(|error| error.to_string())?;
            account_value(value, values.len(), &mut bytes, limits)?;
            values.push(value.to_string());
        }
        Ok(values)
    })
}

pub fn query_strings(url: &str, sql: &str) -> Result<Vec<String>, String> {
    query_strings_with_limits(url, sql, &[], configured_query_limits()?)
}

/// Query with positional string bind parameters.
pub fn query_strings_params(
    url: &str,
    sql: &str,
    params: &[String],
) -> Result<Vec<String>, String> {
    query_strings_with_limits(url, sql, params, configured_query_limits()?)
}

fn query_one_string_with_limits(
    url: &str,
    sql: &str,
    params: &[String],
    limits: QueryLimits,
) -> Result<Option<String>, String> {
    let config = configured_sqlx()?;
    let pool = pool_for(url, config)?;
    let sql = sql.to_owned();
    let params = params.to_vec();
    run_with_deadline(url, "query", config.query_timeout, async move {
        let mut query = sqlx::query(&sql);
        for param in &params {
            query = query.bind(param);
        }
        let row = query
            .fetch_optional(&pool)
            .await
            .map_err(|error| error.to_string())?;
        let Some(row) = row else {
            return Ok(None);
        };
        let value: &str = row.try_get(0).map_err(|error| error.to_string())?;
        let mut bytes = 0;
        account_value(value, 0, &mut bytes, limits)?;
        Ok(Some(value.to_string()))
    })
}

pub fn query_one_string(url: &str, sql: &str) -> Result<Option<String>, String> {
    query_one_string_with_limits(url, sql, &[], configured_query_limits()?)
}

/// Query one row with positional string bind parameters.
pub fn query_one_string_params(
    url: &str,
    sql: &str,
    params: &[String],
) -> Result<Option<String>, String> {
    query_one_string_with_limits(url, sql, params, configured_query_limits()?)
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, MutexGuard, OnceLock};

    fn test_guard() -> MutexGuard<'static, ()> {
        static TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
        TEST_MUTEX
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("test lock")
    }

    fn sqlite_url(tag: &str) -> (std::path::PathBuf, String) {
        let path = std::env::temp_dir().join(format!("rss-sqlx-{tag}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let url = format!("sqlite://{}?mode=rwc", path.display());
        (path, url)
    }

    #[test]
    fn executes_and_queries_over_sqlite() {
        let _guard = test_guard();
        let (path, url) = sqlite_url("basic");

        super::execute(
            &url,
            "create table item(name text); insert into item values ('a'), ('b');",
        )
        .expect("sqlite setup should work");

        assert_eq!(
            super::query_strings(&url, "select name from item order by name")
                .expect("query should work"),
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(
            super::query_one_string(&url, "select name from item order by name")
                .expect("query should work"),
            Some("a".to_string())
        );

        super::close(&url).expect("pool should close");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn binds_string_parameters_without_sql_interpolation() {
        let _guard = test_guard();
        let (path, url) = sqlite_url("params");
        super::execute(&url, "create table item(name text)").expect("sqlite setup should work");

        let name = "a quote: ' and ?".to_string();
        super::execute_params(
            &url,
            "insert into item(name) values (?)",
            std::slice::from_ref(&name),
        )
        .expect("parameterized insert should work");

        assert_eq!(
            super::query_one_string_params(
                &url,
                "select name from item where name = ?",
                std::slice::from_ref(&name),
            )
            .expect("parameterized query should work"),
            Some(name)
        );

        super::close(&url).expect("pool should close");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn enforces_row_and_byte_limits_without_fetch_all() {
        let _guard = test_guard();
        let (path, url) = sqlite_url("limits");
        super::execute(
            &url,
            "create table item(name text); insert into item values ('aa'), ('bbb');",
        )
        .expect("sqlite setup should work");

        let row_error = super::query_strings_with_limits(
            &url,
            "select name from item order by name",
            &[],
            super::QueryLimits {
                max_rows: 1,
                max_bytes: 100,
            },
        )
        .expect_err("two rows must exceed the limit");
        assert!(row_error.ends_with("query result exceeds row limit of 1"));

        let byte_error = super::query_strings_with_limits(
            &url,
            "select name from item order by name",
            &[],
            super::QueryLimits {
                max_rows: 10,
                max_bytes: 4,
            },
        )
        .expect_err("five bytes must exceed the limit");
        assert!(byte_error.ends_with("query result exceeds byte limit of 4"));

        super::close(&url).expect("pool should close");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn pool_identity_uses_the_normalized_url_not_only_its_fingerprint() {
        let first =
            "POSTGRESQL://user:password@DB.EXAMPLE:5432/app?sslmode=require&application_name=rss";
        let second = "postgres://user:password@db.example/app?application_name=rss&sslmode=require";

        assert_eq!(super::pool_key(first), super::pool_key(second));
        assert!(super::pool_key(first).contains("user:password"));
        assert_eq!(
            super::url_fingerprint(first),
            super::url_fingerprint(second)
        );
    }

    #[test]
    fn redacts_structured_url_secrets_from_partial_errors() {
        let url =
            "postgres://private-user:private-password@db.example/app?token=secret&mode=require";
        let error = super::redact_error(
            url,
            "authentication rejected private-password; token secret; mode require",
        );

        assert!(!error.contains("private-password"));
        assert!(!error.contains("secret"));
        assert!(!error.contains("require"));
        assert!(error.contains("<redacted>"));
    }

    #[test]
    fn runtime_work_is_reused_on_one_fixed_worker() {
        let thread_ids = (0..8)
            .map(|_| {
                super::run_on_runtime(async { std::thread::current().id() })
                    .expect("runtime job should run")
            })
            .collect::<Vec<_>>();

        assert!(thread_ids.windows(2).all(|ids| ids[0] == ids[1]));
    }

    #[test]
    fn runtime_queue_full_fails_fast_with_a_clear_error() {
        let (sender, _receiver): (
            std::sync::mpsc::SyncSender<super::RuntimeJob>,
            std::sync::mpsc::Receiver<super::RuntimeJob>,
        ) = std::sync::mpsc::sync_channel(1);
        sender
            .try_send(Box::new(|_| {}))
            .expect("first job fills the queue");

        let error = super::submit_runtime_job(&sender, Box::new(|_| {}))
            .expect_err("a full queue must reject work");

        assert_eq!(
            error,
            format!(
                "SQLx runtime queue is full (capacity {})",
                super::RUNTIME_QUEUE_CAPACITY
            )
        );
    }

    #[test]
    fn bounds_registry_with_lru_eviction_and_closes_pools_deterministically() {
        let _guard = test_guard();
        super::close_all().expect("registry cleanup should work");
        let (path_a, url_a) = sqlite_url("pool-a");
        let (path_b, url_b) = sqlite_url("pool-b");
        let (path_c, url_c) = sqlite_url("pool-c");

        let idle_timeout = std::time::Duration::from_secs(60);
        let connect_timeout = std::time::Duration::from_secs(1);
        super::pool_for_with_config(&url_a, 2, idle_timeout, connect_timeout).expect("first pool");
        super::pool_for_with_config(&url_b, 2, idle_timeout, connect_timeout).expect("second pool");
        super::pool_for_with_config(&url_a, 2, idle_timeout, connect_timeout)
            .expect("existing pool remains available");
        super::pool_for_with_config(&url_c, 2, idle_timeout, connect_timeout)
            .expect("third pool should evict the least recently used pool");

        {
            let registry = super::pools().lock().expect("pool registry lock");
            assert_eq!(registry.entries.len(), 2);
            assert!(registry.entries.contains_key(&super::pool_key(&url_a)));
            assert!(!registry.entries.contains_key(&super::pool_key(&url_b)));
            assert!(registry.entries.contains_key(&super::pool_key(&url_c)));
        }

        super::close(&url_a).expect("single pool close should work");
        {
            let registry = super::pools().lock().expect("pool registry lock");
            assert!(!registry.entries.contains_key(&super::pool_key(&url_a)));
            assert_eq!(registry.entries.len(), 1);
        }
        super::close_all().expect("all pools should close");
        assert!(
            super::pools()
                .lock()
                .expect("pool registry lock")
                .entries
                .is_empty()
        );

        let _ = std::fs::remove_file(path_a);
        let _ = std::fs::remove_file(path_b);
        let _ = std::fs::remove_file(path_c);
    }

    #[test]
    fn evicts_idle_pool_entries() {
        let _guard = test_guard();
        super::close_all().expect("registry cleanup should work");
        let (path_a, url_a) = sqlite_url("idle-a");
        let idle_timeout = std::time::Duration::from_millis(1);
        let connect_timeout = std::time::Duration::from_secs(1);

        let expired = super::pool_for_with_config(&url_a, 2, idle_timeout, connect_timeout)
            .expect("first pool");
        std::thread::sleep(std::time::Duration::from_millis(5));
        let replacement = super::pool_for_with_config(&url_a, 2, idle_timeout, connect_timeout)
            .expect("expired pool should be replaced");
        super::run_on_runtime(async move { expired.close().await })
            .expect("expired pool should close");
        assert!(!replacement.is_closed());

        let registry = super::pools().lock().expect("pool registry lock");
        assert_eq!(registry.entries.len(), 1);
        assert!(registry.entries.contains_key(&super::pool_key(&url_a)));
        drop(registry);

        super::close_all().expect("registry cleanup should work");
        let _ = std::fs::remove_file(path_a);
    }

    #[test]
    fn reports_connection_errors_as_strings() {
        let _guard = test_guard();
        let url = "postgres://private-user:private-password@127.0.0.1:1/missing?token=secret";
        let error =
            super::execute(url, "select 1").expect_err("connecting to a dead postgres should fail");
        assert!(error.contains("<database-url:"));
        assert!(!error.contains(url));
        assert!(!error.contains("private-password"));
        assert!(!error.contains("token=secret"));
        super::close_all().expect("pool cleanup should work");
    }

    #[test]
    fn enforces_total_deadlines_without_exposing_urls() {
        let _guard = test_guard();
        let url = "postgres://deadline-user:deadline-password@example.invalid/database";
        let error =
            super::run_with_deadline(url, "query", std::time::Duration::from_millis(1), async {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                Ok::<_, String>(())
            })
            .expect_err("slow operation should time out");

        assert!(error.contains("timed out after 1ms"));
        assert!(error.contains("<database-url:"));
        assert!(!error.contains(url));
        assert!(!error.contains("deadline-password"));
    }

    #[test]
    fn works_when_called_from_an_existing_tokio_runtime() {
        let _guard = test_guard();
        let (path, url) = sqlite_url("nested-runtime");
        let caller_runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("caller runtime");

        caller_runtime.block_on(async {
            super::execute(
                &url,
                "create table item(name text); insert into item values ('inside-runtime');",
            )
            .expect("sync SQLx adapter should work inside a Tokio runtime");
            assert_eq!(
                super::query_one_string(&url, "select name from item").expect("query should work"),
                Some("inside-runtime".to_string())
            );
        });

        super::close(&url).expect("pool should close");
        let _ = std::fs::remove_file(path);
    }

    // Live Postgres test. Skipped unless `RSS_SQLX_TEST_POSTGRES_URL` points at a
    // reachable database, so CI without a server still passes. Run with e.g.:
    //   RSS_SQLX_TEST_POSTGRES_URL=postgres://user:pw@localhost/db cargo test
    #[test]
    fn executes_and_queries_over_postgres() {
        let _guard = test_guard();
        let Ok(url) = std::env::var("RSS_SQLX_TEST_POSTGRES_URL") else {
            eprintln!("skipping postgres test: RSS_SQLX_TEST_POSTGRES_URL not set");
            return;
        };

        super::execute(&url, "drop table if exists rss_sqlx_item").expect("drop table should work");
        super::execute(
            &url,
            "create table rss_sqlx_item(name text); insert into rss_sqlx_item values ('a'), ('b');",
        )
        .expect("postgres setup should work");

        assert_eq!(
            super::query_strings(&url, "select name from rss_sqlx_item order by name")
                .expect("query should work"),
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(
            super::query_one_string(&url, "select name from rss_sqlx_item order by name")
                .expect("query should work"),
            Some("a".to_string())
        );

        let _ = super::execute(&url, "drop table if exists rss_sqlx_item");
        super::close(&url).expect("pool should close");
    }
}
