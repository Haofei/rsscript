use std::collections::HashMap;
use std::sync::{Mutex, Once, OnceLock};

use sqlx::any::AnyPoolOptions;
use sqlx::{AnyPool, Row};
use tokio::runtime::{Builder, Runtime};

static INSTALL_DRIVERS: Once = Once::new();

pub const DEFAULT_MAX_CONNECTIONS: u32 = 8;
pub const DEFAULT_MAX_CACHED_POOLS: usize = 32;
pub const DEFAULT_MAX_RESULT_ROWS: usize = 10_000;
pub const DEFAULT_MAX_RESULT_BYTES: usize = 16 * 1024 * 1024;

const MAX_CACHED_POOLS_ENV: &str = "RSS_SQLX_MAX_CACHED_POOLS";
const MAX_RESULT_ROWS_ENV: &str = "RSS_SQLX_MAX_RESULT_ROWS";
const MAX_RESULT_BYTES_ENV: &str = "RSS_SQLX_MAX_RESULT_BYTES";

#[derive(Clone, Copy)]
struct QueryLimits {
    max_rows: usize,
    max_bytes: usize,
}

struct PoolEntry {
    pool: AnyPool,
}

#[derive(Default)]
struct PoolRegistry {
    entries: HashMap<String, PoolEntry>,
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

fn configured_query_limits() -> Result<QueryLimits, String> {
    Ok(QueryLimits {
        max_rows: configured_limit(MAX_RESULT_ROWS_ENV, DEFAULT_MAX_RESULT_ROWS)?,
        max_bytes: configured_limit(MAX_RESULT_BYTES_ENV, DEFAULT_MAX_RESULT_BYTES)?,
    })
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

/// Shared Tokio runtime that drives every SQLx future.
fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime for sqlx pool")
    })
}

fn pools() -> &'static Mutex<PoolRegistry> {
    static POOLS: OnceLock<Mutex<PoolRegistry>> = OnceLock::new();
    POOLS.get_or_init(|| Mutex::new(PoolRegistry::default()))
}

fn close_pools(mut evicted: Vec<(String, AnyPool)>) {
    evicted.sort_by(|(left, _), (right, _)| left.cmp(right));
    runtime().block_on(async move {
        for (_, pool) in evicted {
            pool.close().await;
        }
    });
}

fn pool_for_with_limit(url: &str, max_cached_pools: usize) -> Result<AnyPool, String> {
    if max_cached_pools == 0 {
        return Err("maximum cached SQLx pools must be positive".to_string());
    }

    INSTALL_DRIVERS.call_once(sqlx::any::install_default_drivers);
    let pool = {
        let mut registry = pools().lock().map_err(|error| error.to_string())?;
        if let Some(entry) = registry.entries.get(url) {
            return Ok(entry.pool.clone());
        }
        if registry.entries.len() >= max_cached_pools {
            return Err(format!(
                "SQLx pool cache limit of {max_cached_pools} reached; close an unused pool before using a new URL"
            ));
        }

        // Creating the lazy pool while holding the registry lock prevents
        // duplicate entries when native calls arrive concurrently.
        let pool = {
            let _enter = runtime().enter();
            AnyPoolOptions::new()
                .max_connections(DEFAULT_MAX_CONNECTIONS)
                .connect_lazy(url)
                .map_err(|error| error.to_string())?
        };
        registry
            .entries
            .insert(url.to_string(), PoolEntry { pool: pool.clone() });
        pool
    };
    Ok(pool)
}

fn pool_for(url: &str) -> Result<AnyPool, String> {
    static MAX_CACHED_POOLS: OnceLock<Result<usize, String>> = OnceLock::new();
    let limit = MAX_CACHED_POOLS
        .get_or_init(|| configured_limit(MAX_CACHED_POOLS_ENV, DEFAULT_MAX_CACHED_POOLS))
        .clone()?;
    pool_for_with_limit(url, limit)
}

/// Remove and close the cached pool for one URL. In-flight users may delay
/// completion until they return their connections.
pub fn close(url: &str) -> Result<(), String> {
    let removed = pools()
        .lock()
        .map_err(|error| error.to_string())?
        .entries
        .remove(url)
        .map(|entry| (url.to_string(), entry.pool));
    if let Some(pool) = removed {
        close_pools(vec![pool]);
    }
    Ok(())
}

/// Remove and close every cached pool in deterministic URL order.
pub fn close_all() -> Result<(), String> {
    let removed = {
        let mut registry = pools().lock().map_err(|error| error.to_string())?;
        registry
            .entries
            .drain()
            .map(|(url, entry)| (url, entry.pool))
            .collect()
    };
    close_pools(removed);
    Ok(())
}

/// Run one or more SQL statements that produce no rows (DDL, `INSERT`, ...).
pub fn execute(url: &str, sql: &str) -> Result<(), String> {
    let pool = pool_for(url)?;
    runtime().block_on(async move {
        sqlx::raw_sql(sql)
            .execute(&pool)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    })
}

/// Run one SQL statement with positional string bind parameters.
pub fn execute_params(url: &str, sql: &str, params: &[String]) -> Result<(), String> {
    let pool = pool_for(url)?;
    runtime().block_on(async move {
        let mut query = sqlx::query(sql);
        for param in params {
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
    let pool = pool_for(url)?;
    runtime().block_on(async move {
        let mut query = sqlx::query(sql);
        for param in params {
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
    let pool = pool_for(url)?;
    runtime().block_on(async move {
        let mut query = sqlx::query(sql);
        for param in params {
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
        assert_eq!(row_error, "query result exceeds row limit of 1");

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
        assert_eq!(byte_error, "query result exceeds byte limit of 4");

        super::close(&url).expect("pool should close");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn bounds_registry_and_closes_pools_deterministically() {
        let _guard = test_guard();
        super::close_all().expect("registry cleanup should work");
        let (path_a, url_a) = sqlite_url("pool-a");
        let (path_b, url_b) = sqlite_url("pool-b");
        let (path_c, url_c) = sqlite_url("pool-c");

        super::pool_for_with_limit(&url_a, 2).expect("first pool");
        super::pool_for_with_limit(&url_b, 2).expect("second pool");
        super::pool_for_with_limit(&url_a, 2).expect("existing pool remains available");
        let error =
            super::pool_for_with_limit(&url_c, 2).expect_err("third pool must exceed the limit");
        assert_eq!(
            error,
            "SQLx pool cache limit of 2 reached; close an unused pool before using a new URL"
        );

        {
            let registry = super::pools().lock().expect("pool registry lock");
            assert_eq!(registry.entries.len(), 2);
            assert!(registry.entries.contains_key(&url_a));
            assert!(registry.entries.contains_key(&url_b));
            assert!(!registry.entries.contains_key(&url_c));
        }

        super::close(&url_a).expect("single pool close should work");
        {
            let registry = super::pools().lock().expect("pool registry lock");
            assert!(!registry.entries.contains_key(&url_a));
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
    fn reports_connection_errors_as_strings() {
        let _guard = test_guard();
        let error = super::execute("postgres://127.0.0.1:1/missing", "select 1")
            .expect_err("connecting to a dead postgres should fail");
        assert!(!error.is_empty());
        super::close_all().expect("pool cleanup should work");
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
