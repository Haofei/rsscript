use std::collections::HashMap;
use std::sync::{Mutex, Once, OnceLock};

use sqlx::any::AnyPoolOptions;
use sqlx::{AnyPool, Row};
use tokio::runtime::{Builder, Runtime};

static INSTALL_DRIVERS: Once = Once::new();

/// Maximum number of connections held open per pool (per connection URL).
const MAX_CONNECTIONS: u32 = 8;

/// Shared Tokio runtime that drives every sqlx future. A single runtime lives
/// for the lifetime of the process so the cached pools stay valid and reuse
/// their connections across calls.
fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime for sqlx pool")
    })
}

/// Process-wide registry of connection pools, keyed by connection URL.
fn pools() -> &'static Mutex<HashMap<String, AnyPool>> {
    static POOLS: OnceLock<Mutex<HashMap<String, AnyPool>>> = OnceLock::new();
    POOLS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Return a cloneable handle to the shared connection pool for `url`, creating
/// it lazily on first use. Pools are cached process-wide and keyed by URL, so
/// repeated `native fn` calls reuse the same connections instead of opening a
/// brand new connection (and runtime) every time. The clone is cheap: `AnyPool`
/// is reference counted internally.
fn pool_for(url: &str) -> Result<AnyPool, String> {
    INSTALL_DRIVERS.call_once(sqlx::any::install_default_drivers);
    let mut guard = pools().lock().map_err(|error| error.to_string())?;
    if let Some(pool) = guard.get(url) {
        return Ok(pool.clone());
    }
    // Build the pool inside the runtime context so any background tasks it
    // spawns attach to our shared runtime. `connect_lazy` does not open a
    // connection yet, so connection failures surface on the first query.
    let pool = {
        let _enter = runtime().enter();
        AnyPoolOptions::new()
            .max_connections(MAX_CONNECTIONS)
            .connect_lazy(url)
            .map_err(|error| error.to_string())?
    };
    guard.insert(url.to_string(), pool.clone());
    Ok(pool)
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

/// Run a query and collect the first column of every row as a string.
pub fn query_strings(url: &str, sql: &str) -> Result<Vec<String>, String> {
    let pool = pool_for(url)?;
    runtime().block_on(async move {
        let rows = sqlx::query(sql)
            .fetch_all(&pool)
            .await
            .map_err(|error| error.to_string())?;
        let mut values = Vec::with_capacity(rows.len());
        for row in &rows {
            let value: String = row.try_get(0).map_err(|error| error.to_string())?;
            values.push(value);
        }
        Ok(values)
    })
}

/// Run a query and return the first column of the first row, if any.
pub fn query_one_string(url: &str, sql: &str) -> Result<Option<String>, String> {
    let values = query_strings(url, sql)?;
    Ok(values.into_iter().next())
}

#[cfg(test)]
mod tests {
    fn sqlite_url(tag: &str) -> (std::path::PathBuf, String) {
        let path = std::env::temp_dir().join(format!("rss-sqlx-{tag}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let url = format!("sqlite://{}?mode=rwc", path.display());
        (path, url)
    }

    #[test]
    fn executes_and_queries_over_sqlite() {
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

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn reuses_a_single_pool_per_url() {
        let (path, url) = sqlite_url("pool");

        super::execute(&url, "create table t(name text); insert into t values ('x');")
            .expect("sqlite setup should work");

        // Many calls against the same URL must reuse one cached pool rather than
        // creating a new connection per call.
        for _ in 0..16 {
            assert_eq!(
                super::query_one_string(&url, "select name from t").expect("query should work"),
                Some("x".to_string())
            );
        }

        let guard = super::pools().lock().expect("pool registry lock");
        assert_eq!(
            guard.keys().filter(|key| *key == &url).count(),
            1,
            "exactly one pool should be cached for the URL"
        );
        drop(guard);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn reports_connection_errors_as_strings() {
        let error = super::execute("postgres://127.0.0.1:1/missing", "select 1")
            .expect_err("connecting to a dead postgres should fail");
        assert!(!error.is_empty());
    }

    // Live Postgres test. Skipped unless `RSS_SQLX_TEST_POSTGRES_URL` points at a
    // reachable database, so CI without a server still passes. Run with e.g.:
    //   RSS_SQLX_TEST_POSTGRES_URL=postgres://user:pw@localhost/db cargo test
    #[test]
    fn executes_and_queries_over_postgres() {
        let Ok(url) = std::env::var("RSS_SQLX_TEST_POSTGRES_URL") else {
            eprintln!("skipping postgres test: RSS_SQLX_TEST_POSTGRES_URL not set");
            return;
        };

        super::execute(&url, "drop table if exists rss_sqlx_item")
            .expect("drop table should work");
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
    }
}
