use std::path::PathBuf;

use rusqlite::Connection;

pub fn execute(path: &PathBuf, sql: &str) -> Result<(), String> {
    let conn = Connection::open(path).map_err(|error| error.to_string())?;
    conn.execute_batch(sql).map_err(|error| error.to_string())
}

pub fn query_strings(path: &PathBuf, sql: &str) -> Result<Vec<String>, String> {
    let conn = Connection::open(path).map_err(|error| error.to_string())?;
    let mut statement = conn.prepare(sql).map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?;
    let mut values = Vec::new();
    for row in rows {
        values.push(row.map_err(|error| error.to_string())?);
    }
    Ok(values)
}

pub fn query_one_string(path: &PathBuf, sql: &str) -> Result<Option<String>, String> {
    let values = query_strings(path, sql)?;
    Ok(values.into_iter().next())
}

#[cfg(test)]
mod tests {
    #[test]
    fn executes_and_queries_strings() {
        let path = std::env::temp_dir().join(format!("rss-sqlite-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);

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
}
