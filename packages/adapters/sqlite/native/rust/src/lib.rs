use std::path::PathBuf;

use rusqlite::{Connection, OptionalExtension, params_from_iter};

pub const DEFAULT_MAX_RESULT_ROWS: usize = 10_000;
pub const DEFAULT_MAX_RESULT_BYTES: usize = 16 * 1024 * 1024;

const MAX_RESULT_ROWS_ENV: &str = "RSS_SQLITE_MAX_RESULT_ROWS";
const MAX_RESULT_BYTES_ENV: &str = "RSS_SQLITE_MAX_RESULT_BYTES";

#[derive(Clone, Copy)]
struct QueryLimits {
    max_rows: usize,
    max_bytes: usize,
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

fn query_strings_with_limits(
    path: &PathBuf,
    sql: &str,
    params: &[String],
    limits: QueryLimits,
) -> Result<Vec<String>, String> {
    let conn = Connection::open(path).map_err(|error| error.to_string())?;
    let mut statement = conn.prepare(sql).map_err(|error| error.to_string())?;
    let mut rows = statement
        .query(params_from_iter(params))
        .map_err(|error| error.to_string())?;
    let mut values = Vec::new();
    let mut bytes = 0;
    while let Some(row) = rows.next().map_err(|error| error.to_string())? {
        let value: String = row.get(0).map_err(|error| error.to_string())?;
        account_value(&value, values.len(), &mut bytes, limits)?;
        values.push(value);
    }
    Ok(values)
}

fn query_one_string_with_limits(
    path: &PathBuf,
    sql: &str,
    params: &[String],
    limits: QueryLimits,
) -> Result<Option<String>, String> {
    let conn = Connection::open(path).map_err(|error| error.to_string())?;
    let mut statement = conn.prepare(sql).map_err(|error| error.to_string())?;
    let value = statement
        .query_row(params_from_iter(params), |row| row.get::<_, String>(0))
        .optional()
        .map_err(|error| error.to_string())?;
    if let Some(value) = &value {
        let mut bytes = 0;
        account_value(value, 0, &mut bytes, limits)?;
    }
    Ok(value)
}

/// Run one or more SQL statements without bind parameters.
pub fn execute(path: &PathBuf, sql: &str) -> Result<(), String> {
    let conn = Connection::open(path).map_err(|error| error.to_string())?;
    conn.execute_batch(sql).map_err(|error| error.to_string())
}

/// Run one SQL statement with positional string bind parameters.
pub fn execute_params(path: &PathBuf, sql: &str, params: &[String]) -> Result<(), String> {
    let conn = Connection::open(path).map_err(|error| error.to_string())?;
    conn.execute(sql, params_from_iter(params))
        .map(|_| ())
        .map_err(|error| error.to_string())
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
    fn sqlite_path(tag: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("rss-sqlite-{tag}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn executes_and_queries_strings() {
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
}
