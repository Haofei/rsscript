use std::sync::Once;

use sqlx::{AnyConnection, Connection, Row};
use tokio::runtime::Builder;

static INSTALL_DRIVERS: Once = Once::new();

/// Run a sqlx future to completion on a fresh current-thread Tokio runtime, so
/// the async sqlx API can be exposed through synchronous `native fn` bindings.
/// sqlx's `Any` backend drivers are installed once on first use.
fn block_on<T, F>(future: F) -> Result<T, String>
where
    F: std::future::Future<Output = Result<T, String>>,
{
    INSTALL_DRIVERS.call_once(sqlx::any::install_default_drivers);
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(future)
}

/// Run one or more SQL statements that produce no rows (DDL, `INSERT`, ...).
pub fn execute(url: &str, sql: &str) -> Result<(), String> {
    block_on(async {
        let mut conn = AnyConnection::connect(url)
            .await
            .map_err(|error| error.to_string())?;
        let result = sqlx::raw_sql(sql)
            .execute(&mut conn)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string());
        let _ = conn.close().await;
        result
    })
}

/// Run a query and collect the first column of every row as a string.
pub fn query_strings(url: &str, sql: &str) -> Result<Vec<String>, String> {
    block_on(async {
        let mut conn = AnyConnection::connect(url)
            .await
            .map_err(|error| error.to_string())?;
        let result = collect_first_column(&mut conn, sql).await;
        let _ = conn.close().await;
        result
    })
}

/// Run a query and return the first column of the first row, if any.
pub fn query_one_string(url: &str, sql: &str) -> Result<Option<String>, String> {
    let values = query_strings(url, sql)?;
    Ok(values.into_iter().next())
}

async fn collect_first_column(conn: &mut AnyConnection, sql: &str) -> Result<Vec<String>, String> {
    let rows = sqlx::query(sql)
        .fetch_all(conn)
        .await
        .map_err(|error| error.to_string())?;
    let mut values = Vec::with_capacity(rows.len());
    for row in &rows {
        let value: String = row.try_get(0).map_err(|error| error.to_string())?;
        values.push(value);
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    #[test]
    fn executes_and_queries_over_sqlite() {
        let path = std::env::temp_dir().join(format!("rss-sqlx-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let url = format!("sqlite://{}?mode=rwc", path.display());

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
    fn reports_connection_errors_as_strings() {
        let error = super::execute("postgres://127.0.0.1:1/missing", "select 1")
            .expect_err("connecting to a dead postgres should fail");
        assert!(!error.is_empty());
    }
}
