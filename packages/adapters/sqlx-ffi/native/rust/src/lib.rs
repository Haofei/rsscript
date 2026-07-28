use std::collections::HashMap;
use std::future::Future;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, Once, OnceLock};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use sqlx::any::AnyPoolOptions;
use sqlx::{AnyPool, Row};
use tokio::runtime::{Builder, Runtime};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

static INSTALL_DRIVERS: Once = Once::new();

pub const DEFAULT_MAX_CONNECTIONS: u32 = 8;
pub const DEFAULT_MAX_CACHED_POOLS: usize = 32;
pub const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 5_000;
pub const DEFAULT_QUERY_TIMEOUT_MS: u64 = 30_000;
pub const DEFAULT_POOL_IDLE_TIMEOUT_MS: u64 = 300_000;
pub const DEFAULT_MAX_RESULT_ROWS: usize = 10_000;
pub const DEFAULT_MAX_RESULT_BYTES: usize = 16 * 1024 * 1024;
pub const DEFAULT_MAX_IN_FLIGHT: usize = 64;
pub const DEFAULT_MAX_IN_FLIGHT_PER_POOL: usize = DEFAULT_MAX_CONNECTIONS as usize;

const MAX_CACHED_POOLS_ENV: &str = "RSS_SQLX_MAX_CACHED_POOLS";
const CONNECT_TIMEOUT_MS_ENV: &str = "RSS_SQLX_CONNECT_TIMEOUT_MS";
const QUERY_TIMEOUT_MS_ENV: &str = "RSS_SQLX_QUERY_TIMEOUT_MS";
const POOL_IDLE_TIMEOUT_MS_ENV: &str = "RSS_SQLX_POOL_IDLE_TIMEOUT_MS";
const MAX_RESULT_ROWS_ENV: &str = "RSS_SQLX_MAX_RESULT_ROWS";
const MAX_RESULT_BYTES_ENV: &str = "RSS_SQLX_MAX_RESULT_BYTES";
const MAX_IN_FLIGHT_ENV: &str = "RSS_SQLX_MAX_IN_FLIGHT";
const MAX_IN_FLIGHT_PER_POOL_ENV: &str = "RSS_SQLX_MAX_IN_FLIGHT_PER_POOL";

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
    max_in_flight_per_pool: usize,
}

type PoolKey = String;

#[derive(Clone)]
struct PoolHandle {
    pool: AnyPool,
    permits: Arc<Semaphore>,
}

struct PoolEntry {
    handle: PoolHandle,
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
        max_in_flight_per_pool: configured_limit(
            MAX_IN_FLIGHT_PER_POOL_ENV,
            DEFAULT_MAX_IN_FLIGHT_PER_POOL,
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

    format!("{scheme}://{authority}{suffix}")
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
    let endpoint = redacted_database_endpoint(url);
    let digest = Sha256::digest(endpoint.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
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
    if let Some((userinfo, _)) = authority.rsplit_once('@') {
        if let Some((_, password)) = userinfo.split_once(':')
            && !password.is_empty()
        {
            secrets.push(password.to_owned());
            if let Some(decoded) = percent_decode(password)
                && decoded != password
            {
                secrets.push(decoded);
            }
        }
        if !userinfo.is_empty() {
            secrets.push(userinfo.to_owned());
        }
    }
    if let Some((query, _)) = suffix
        .split_once('?')
        .map(|(_, query)| query.split_once('#').unwrap_or((query, "")))
    {
        for value in query.split('&').filter_map(|field| {
            let (key, value) = field.split_once('=')?;
            (is_sensitive_query_key(key) && !value.is_empty()).then(|| value.to_owned())
        }) {
            if let Some(decoded) = percent_decode(&value)
                && decoded != value
            {
                secrets.push(decoded);
            }
            secrets.push(value);
        }
    }
    secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    secrets.dedup();
    secrets
}

fn redacted_database_endpoint(url: &str) -> String {
    let normalized = normalize_database_url(url);
    let Some((scheme, rest)) = normalized.split_once("://") else {
        return "<invalid-database-url>".to_owned();
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let path = &rest[authority_end..];
    let path_end = path.find(['?', '#']).unwrap_or(path.len());
    format!("{scheme}://{host}{}", &path[..path_end])
}

fn is_sensitive_query_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "access_token"
            | "api_key"
            | "apikey"
            | "auth"
            | "credential"
            | "password"
            | "secret"
            | "token"
    )
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let high = hex_value(bytes[index + 1])?;
            let low = hex_value(bytes[index + 2])?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
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

struct RuntimeService {
    runtime: Runtime,
    permits: Arc<Semaphore>,
}

/// Shared multi-thread runtime. Work is admitted with a global semaphore and
/// then scheduled independently, so one slow pool cannot serialize all others.
fn runtime_service() -> Result<&'static RuntimeService, String> {
    static SERVICE: OnceLock<Result<RuntimeService, String>> = OnceLock::new();
    SERVICE
        .get_or_init(|| {
            let max_in_flight = configured_limit(MAX_IN_FLIGHT_ENV, DEFAULT_MAX_IN_FLIGHT)?;
            let runtime = Builder::new_multi_thread()
                .thread_name("rss-sqlx-runtime")
                .enable_all()
                .build()
                .map_err(|error| format!("failed to create SQLx runtime: {error}"))?;
            Ok(RuntimeService {
                runtime,
                permits: Arc::new(Semaphore::new(max_in_flight)),
            })
        })
        .as_ref()
        .map_err(Clone::clone)
}

fn try_acquire(permits: &Arc<Semaphore>, scope: &str) -> Result<OwnedSemaphorePermit, String> {
    Arc::clone(permits)
        .try_acquire_owned()
        .map_err(|_| format!("SQLx {scope} concurrency limit reached"))
}

fn run_on_runtime<F, T>(future: F) -> Result<T, String>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let service = runtime_service()?;
    let global_permit = try_acquire(&service.permits, "global")?;
    let (sender, receiver) = mpsc::sync_channel(1);
    service.runtime.spawn(async move {
        let result = tokio::spawn(future)
            .await
            .map_err(|_| "SQLx runtime task panicked".to_string());
        drop(global_permit);
        let _ = sender.send(result);
    });
    receiver
        .recv()
        .map_err(|_| "SQLx runtime task stopped".to_string())?
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

fn run_pool_with_deadline<F, T>(
    url: &str,
    operation: &'static str,
    timeout: Duration,
    permits: Arc<Semaphore>,
    future: F,
) -> Result<T, String>
where
    F: Future<Output = Result<T, String>> + Send + 'static,
    T: Send + 'static,
{
    let pool_permit = try_acquire(&permits, "per-pool")?;
    run_with_deadline(url, operation, timeout, async move {
        let result = future.await;
        drop(pool_permit);
        result
    })
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
    max_in_flight_per_pool: usize,
) -> Result<PoolHandle, String> {
    if max_cached_pools == 0 {
        return Err("maximum cached SQLx pools must be positive".to_string());
    }
    if max_in_flight_per_pool == 0 {
        return Err("maximum in-flight SQLx operations per pool must be positive".to_string());
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
        return Ok(entry.handle.clone());
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
        let _enter = runtime_service()?.runtime.enter();
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
    let handle = PoolHandle {
        pool,
        permits: Arc::new(Semaphore::new(max_in_flight_per_pool)),
    };
    registry.entries.insert(
        key,
        PoolEntry {
            handle: handle.clone(),
            last_used: now,
            sequence,
        },
    );
    Ok(handle)
}

fn pool_for(url: &str, config: SqlxConfig) -> Result<PoolHandle, String> {
    pool_for_with_config(
        url,
        config.max_cached_pools,
        config.pool_idle_timeout,
        config.connect_timeout,
        config.max_in_flight_per_pool,
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
        .map(|entry| (key, entry.handle.pool));
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
            .map(|(key, entry)| (key, entry.handle.pool))
            .collect()
    };
    close_pools(removed)
}

/// Run one or more SQL statements that produce no rows (DDL, `INSERT`, ...).
pub fn execute(url: &str, sql: &str) -> Result<(), String> {
    let config = configured_sqlx()?;
    let handle = pool_for(url, config)?;
    let pool = handle.pool.clone();
    let sql = sql.to_owned();
    run_pool_with_deadline(
        url,
        "query",
        config.query_timeout,
        handle.permits,
        async move {
            sqlx::raw_sql(&sql)
                .execute(&pool)
                .await
                .map(|_| ())
                .map_err(|error| error.to_string())
        },
    )
}

/// Run one SQL statement with positional string bind parameters.
pub fn execute_params(url: &str, sql: &str, params: &[String]) -> Result<(), String> {
    let config = configured_sqlx()?;
    let handle = pool_for(url, config)?;
    let pool = handle.pool.clone();
    let sql = sql.to_owned();
    let params = params.to_vec();
    run_pool_with_deadline(
        url,
        "query",
        config.query_timeout,
        handle.permits,
        async move {
            let mut query = sqlx::query(&sql);
            for param in &params {
                query = query.bind(param);
            }
            query
                .execute(&pool)
                .await
                .map(|_| ())
                .map_err(|error| error.to_string())
        },
    )
}

fn query_strings_with_limits(
    url: &str,
    sql: &str,
    params: &[String],
    limits: QueryLimits,
) -> Result<Vec<String>, String> {
    let config = configured_sqlx()?;
    let handle = pool_for(url, config)?;
    let pool = handle.pool.clone();
    let sql = sql.to_owned();
    let params = params.to_vec();
    run_pool_with_deadline(
        url,
        "query",
        config.query_timeout,
        handle.permits,
        async move {
            let mut query = sqlx::query(&sql);
            for param in &params {
                query = query.bind(param);
            }
            let mut rows = query.fetch(&pool);
            let mut values = Vec::new();
            let mut bytes = 0;
            while let Some(row) =
                std::future::poll_fn(|context| rows.as_mut().poll_next(context)).await
            {
                let row = row.map_err(|error| error.to_string())?;
                let value: &str = row.try_get(0).map_err(|error| error.to_string())?;
                account_value(value, values.len(), &mut bytes, limits)?;
                values.push(value.to_string());
            }
            Ok(values)
        },
    )
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
    let handle = pool_for(url, config)?;
    let pool = handle.pool.clone();
    let sql = sql.to_owned();
    let params = params.to_vec();
    run_pool_with_deadline(
        url,
        "query",
        config.query_timeout,
        handle.permits,
        async move {
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
        },
    )
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
    fn pool_identity_preserves_query_order_and_uses_full_normalized_url() {
        let first =
            "POSTGRESQL://user:password@DB.EXAMPLE:5432/app?sslmode=require&application_name=rss";
        let equivalent =
            "postgres://user:password@db.example/app?sslmode=require&application_name=rss";
        let reordered =
            "postgres://user:password@db.example/app?application_name=rss&sslmode=require";

        assert_eq!(super::pool_key(first), super::pool_key(equivalent));
        assert_ne!(super::pool_key(first), super::pool_key(reordered));
        assert!(super::pool_key(first).contains("user:password"));
        assert_eq!(
            super::url_fingerprint(first),
            super::url_fingerprint(reordered)
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
        assert!(error.contains("mode require"));
        assert!(error.contains("<redacted>"));
    }

    #[test]
    fn redacts_percent_decoded_passwords_without_hiding_nonsecret_options() {
        let url = "postgres://user:p%40ssword@db.example/app?sslmode=require&token=secret";
        let error = super::redact_error(
            url,
            "authentication rejected p@ssword; token secret; sslmode require",
        );

        assert!(!error.contains("p@ssword"));
        assert!(!error.contains("secret"));
        assert!(error.contains("sslmode require"));
    }

    #[test]
    fn runtime_executes_independent_jobs_concurrently() {
        let started = std::time::Instant::now();
        let jobs = (0..2)
            .map(|_| {
                std::thread::spawn(|| {
                    super::run_on_runtime(async {
                        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
                    })
                    .expect("runtime job should run");
                })
            })
            .collect::<Vec<_>>();
        for job in jobs {
            job.join().expect("caller thread");
        }

        assert!(
            started.elapsed() < std::time::Duration::from_millis(145),
            "independent jobs should overlap instead of running serially"
        );
    }

    #[test]
    fn concurrency_limits_fail_fast_with_a_clear_scope() {
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let held = super::try_acquire(&permits, "per-pool").expect("first permit");
        let error =
            super::try_acquire(&permits, "per-pool").expect_err("second permit must be rejected");
        assert_eq!(error, "SQLx per-pool concurrency limit reached");
        drop(held);
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
        super::pool_for_with_config(&url_a, 2, idle_timeout, connect_timeout, 2)
            .expect("first pool");
        super::pool_for_with_config(&url_b, 2, idle_timeout, connect_timeout, 2)
            .expect("second pool");
        super::pool_for_with_config(&url_a, 2, idle_timeout, connect_timeout, 2)
            .expect("existing pool remains available");
        super::pool_for_with_config(&url_c, 2, idle_timeout, connect_timeout, 2)
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

        let expired = super::pool_for_with_config(&url_a, 2, idle_timeout, connect_timeout, 2)
            .expect("first pool");
        std::thread::sleep(std::time::Duration::from_millis(5));
        let replacement = super::pool_for_with_config(&url_a, 2, idle_timeout, connect_timeout, 2)
            .expect("expired pool should be replaced");
        super::run_on_runtime(async move { expired.pool.close().await })
            .expect("expired pool should close");
        assert!(!replacement.pool.is_closed());

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
