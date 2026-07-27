use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use rusqlite::{Connection, params_from_iter};

pub const DEFAULT_MAX_CACHED_CONNECTIONS: usize = 32;
pub const DEFAULT_BUSY_TIMEOUT_MS: u64 = 5_000;
pub const DEFAULT_MAX_RESULT_ROWS: usize = 10_000;
pub const DEFAULT_MAX_RESULT_BYTES: usize = 16 * 1024 * 1024;

const MAX_CACHED_CONNECTIONS_ENV: &str = "RSS_SQLITE_MAX_CACHED_CONNECTIONS";
const BUSY_TIMEOUT_MS_ENV: &str = "RSS_SQLITE_BUSY_TIMEOUT_MS";
const MAX_RESULT_ROWS_ENV: &str = "RSS_SQLITE_MAX_RESULT_ROWS";
const MAX_RESULT_BYTES_ENV: &str = "RSS_SQLITE_MAX_RESULT_BYTES";

type SharedConnection = Arc<Mutex<Connection>>;

#[derive(Clone, Copy)]
struct QueryLimits {
    max_rows: usize,
    max_bytes: usize,
}

struct ConnectionEntry {
    connection: SharedConnection,
    last_used: u64,
}

#[derive(Default)]
struct ConnectionRegistry {
    entries: HashMap<PathBuf, ConnectionEntry>,
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

fn configured_busy_timeout() -> Result<Duration, String> {
    let timeout_ms = configured_limit(BUSY_TIMEOUT_MS_ENV, DEFAULT_BUSY_TIMEOUT_MS as usize)?;
    Ok(Duration::from_millis(timeout_ms as u64))
}

fn configured_query_limits() -> Result<QueryLimits, String> {
    Ok(QueryLimits {
        max_rows: configured_limit(MAX_RESULT_ROWS_ENV, DEFAULT_MAX_RESULT_ROWS)?,
        max_bytes: configured_limit(MAX_RESULT_BYTES_ENV, DEFAULT_MAX_RESULT_BYTES)?,
    })
}

fn connections() -> &'static Mutex<ConnectionRegistry> {
    static CONNECTIONS: OnceLock<Mutex<ConnectionRegistry>> = OnceLock::new();
    CONNECTIONS.get_or_init(|| Mutex::new(ConnectionRegistry::default()))
}

fn connection_for_with_config(
    path: &PathBuf,
    max_cached_connections: usize,
    busy_timeout: Duration,
) -> Result<SharedConnection, String> {
    if max_cached_connections == 0 {
        return Err("maximum cached SQLite connections must be positive".to_string());
    }

    let mut registry = connections().lock().map_err(|error| error.to_string())?;
    registry.clock = registry.clock.wrapping_add(1);
    let last_used = registry.clock;
    if let Some(entry) = registry.entries.get_mut(path) {
        entry.last_used = last_used;
        return Ok(Arc::clone(&entry.connection));
    }

    let connection = Connection::open(path).map_err(|error| error.to_string())?;
    connection
        .busy_timeout(busy_timeout)
        .map_err(|error| error.to_string())?;
    let connection = Arc::new(Mutex::new(connection));

    while registry.entries.len() >= max_cached_connections {
        let lru_path = registry
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(path, _)| path.clone())
            .expect("a full positive-capacity cache has an entry");
        registry.entries.remove(&lru_path);
    }
    registry.entries.insert(
        path.clone(),
        ConnectionEntry {
            connection: Arc::clone(&connection),
            last_used,
        },
    );
    Ok(connection)
}

fn connection_for(path: &PathBuf) -> Result<SharedConnection, String> {
    static MAX_CACHED_CONNECTIONS: OnceLock<Result<usize, String>> = OnceLock::new();
    let limit = MAX_CACHED_CONNECTIONS
        .get_or_init(|| {
            configured_limit(MAX_CACHED_CONNECTIONS_ENV, DEFAULT_MAX_CACHED_CONNECTIONS)
        })
        .clone()?;
    connection_for_with_config(path, limit, configured_busy_timeout()?)
}

fn with_connection<T>(
    path: &PathBuf,
    operation: impl FnOnce(&mut Connection) -> Result<T, String>,
) -> Result<T, String> {
    let connection = connection_for(path)?;
    let mut connection = connection.lock().map_err(|error| error.to_string())?;
    operation(&mut connection)
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

fn query_strings_with_limits(
    path: &PathBuf,
    sql: &str,
    params: &[String],
    limits: QueryLimits,
) -> Result<Vec<String>, String> {
    with_connection(path, |conn| {
        let mut statement = conn.prepare(sql).map_err(|error| error.to_string())?;
        let mut rows = statement
            .query(params_from_iter(params))
            .map_err(|error| error.to_string())?;
        let mut values = Vec::new();
        let mut bytes = 0;
        while let Some(row) = rows.next().map_err(|error| error.to_string())? {
            let raw = row.get_ref(0).map_err(|error| error.to_string())?;
            let value = raw.as_str().map_err(|error| error.to_string())?;
            account_value(value, values.len(), &mut bytes, limits)?;
            values.push(value.to_string());
        }
        Ok(values)
    })
}

fn query_one_string_with_limits(
    path: &PathBuf,
    sql: &str,
    params: &[String],
    limits: QueryLimits,
) -> Result<Option<String>, String> {
    with_connection(path, |conn| {
        let mut statement = conn.prepare(sql).map_err(|error| error.to_string())?;
        let mut rows = statement
            .query(params_from_iter(params))
            .map_err(|error| error.to_string())?;
        let Some(row) = rows.next().map_err(|error| error.to_string())? else {
            return Ok(None);
        };
        let raw = row.get_ref(0).map_err(|error| error.to_string())?;
        let value = raw.as_str().map_err(|error| error.to_string())?;
        let mut bytes = 0;
        account_value(value, 0, &mut bytes, limits)?;
        Ok(Some(value.to_string()))
    })
}

/// Run one or more SQL statements without bind parameters.
pub fn execute(path: &PathBuf, sql: &str) -> Result<(), String> {
    with_connection(path, |conn| {
        conn.execute_batch(sql).map_err(|error| error.to_string())
    })
}

/// Run one SQL statement with positional string bind parameters.
pub fn execute_params(path: &PathBuf, sql: &str, params: &[String]) -> Result<(), String> {
    with_connection(path, |conn| {
        conn.execute(sql, params_from_iter(params))
            .map(|_| ())
            .map_err(|error| error.to_string())
    })
}

pub fn query_strings(path: &PathBuf, sql: &str) -> Result<Vec<String>, String> {
    query_strings_with_limits(path, sql, &[], configured_query_limits()?)
}

/// Query with positional string bind parameters.
pub fn query_strings_params(
    path: &PathBuf,
    sql: &str,
    params: &[String],
) -> Result<Vec<String>, String> {
    query_strings_with_limits(path, sql, params, configured_query_limits()?)
}

pub fn query_one_string(path: &PathBuf, sql: &str) -> Result<Option<String>, String> {
    query_one_string_with_limits(path, sql, &[], configured_query_limits()?)
}

/// Query one row with positional string bind parameters.
pub fn query_one_string_params(
    path: &PathBuf,
    sql: &str,
    params: &[String],
) -> Result<Option<String>, String> {
    query_one_string_with_limits(path, sql, params, configured_query_limits()?)
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, MutexGuard, OnceLock};
    use std::time::Duration;

    fn test_guard() -> MutexGuard<'static, ()> {
        static TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
        TEST_MUTEX
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("test lock")
    }

    fn sqlite_path(tag: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("rss-sqlite-{tag}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn executes_and_queries_strings() {
        let _guard = test_guard();
        let path = sqlite_path("basic");

        super::execute(
            &path,
            "create table item(name text); insert into item values ('a'), ('b');",
        )
        .expect("sqlite setup should work");

        assert_eq!(
            super::query_strings(&path, "select name from item order by name")
                .expect("query should work"),
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(
            super::query_one_string(&path, "select name from item order by name")
                .expect("query should work"),
            Some("a".to_string())
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn binds_string_parameters_without_sql_interpolation() {
        let _guard = test_guard();
        let path = sqlite_path("params");
        super::execute(&path, "create table item(name text)").expect("sqlite setup should work");

        let name = "a quote: ' and ?".to_string();
        super::execute_params(
            &path,
            "insert into item(name) values (?1)",
            std::slice::from_ref(&name),
        )
        .expect("parameterized insert should work");

        assert_eq!(
            super::query_one_string_params(
                &path,
                "select name from item where name = ?1",
                std::slice::from_ref(&name),
            )
            .expect("parameterized query should work"),
            Some(name)
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn enforces_row_and_byte_limits() {
        let _guard = test_guard();
        let path = sqlite_path("limits");
        super::execute(
            &path,
            "create table item(name text); insert into item values ('aa'), ('bbb');",
        )
        .expect("sqlite setup should work");

        let row_error = super::query_strings_with_limits(
            &path,
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
            &path,
            "select name from item order by name",
            &[],
            super::QueryLimits {
                max_rows: 10,
                max_bytes: 4,
            },
        )
        .expect_err("five bytes must exceed the limit");
        assert_eq!(byte_error, "query result exceeds byte limit of 4");

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn reuses_connections_and_evicts_the_least_recently_used_path() {
        let _guard = test_guard();
        super::connections()
            .lock()
            .expect("connection registry")
            .entries
            .clear();
        let paths = [
            sqlite_path("cache-a"),
            sqlite_path("cache-b"),
            sqlite_path("cache-c"),
        ];
        let timeout = Duration::from_millis(17);

        let first =
            super::connection_for_with_config(&paths[0], 2, timeout).expect("first connection");
        assert_eq!(
            first
                .lock()
                .expect("connection lock")
                .query_row("pragma busy_timeout", [], |row| row.get::<_, u64>(0))
                .expect("busy timeout pragma"),
            17
        );
        super::connection_for_with_config(&paths[1], 2, timeout).expect("second connection");
        let reused = super::connection_for_with_config(&paths[0], 2, timeout)
            .expect("first connection should be reused");
        assert!(std::sync::Arc::ptr_eq(&first, &reused));

        super::connection_for_with_config(&paths[2], 2, timeout).expect("third connection");
        let registry = super::connections().lock().expect("connection registry");
        assert_eq!(registry.entries.len(), 2);
        assert!(registry.entries.contains_key(&paths[0]));
        assert!(!registry.entries.contains_key(&paths[1]));
        assert!(registry.entries.contains_key(&paths[2]));
        drop(registry);

        for path in paths {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn execute_supports_an_explicit_transaction_batch() {
        let _guard = test_guard();
        let path = sqlite_path("transaction");
        super::execute(&path, "create table item(name text)").expect("sqlite setup should work");
        super::execute(
            &path,
            "begin immediate; insert into item values ('a'); insert into item values ('b'); commit;",
        )
        .expect("transaction batch should commit");

        assert_eq!(
            super::query_strings(&path, "select name from item order by name")
                .expect("query should work"),
            vec!["a".to_string(), "b".to_string()]
        );

        let _ = std::fs::remove_file(path);
    }
}
