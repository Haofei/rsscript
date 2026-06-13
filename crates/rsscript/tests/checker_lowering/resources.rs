//! Spec §6.4/§7.3 — resource & with-scope lowering
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn rust_lowering_maps_db_resource_pool_to_runtime_hooks() {
    let source = r#"
features: local

fn run_query(url: read Url, sql: read String) -> Result<Unit, DbError> {
    local pool = ResourcePool<DbConnection>.new(
        create: || DbConnection.open(url: read url),
        max_size: 2,
    )

    with ResourcePool.borrow(pool: mut pool) as conn {
        DbConnection.query(conn: mut conn, sql: read sql)?
    }

    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("db.rss", source).expect("source should lower");

    assert!(rust.contains("url: &String"));
    assert!(rust.contains("-> Result<(), rsscript_runtime::DbError>"));
    assert!(rust.contains("let mut pool = rsscript_runtime::ResourcePool::from_factory(2i64, || rsscript_runtime::db_connection_open(url));"));
    assert!(rust.contains("let mut conn = rsscript_runtime::unwrap_runtime(rsscript_runtime::ResourcePool::borrow_at(&mut pool, rsscript_runtime::SourceSpan::new(\"db.rss\""));
    assert!(rust.contains("rsscript_runtime::db_connection_query(&mut conn, sql)?;"));
}

#[test]
fn rust_lowering_maps_lazy_pool_try_borrow_discard_and_stats() {
    let source = r#"
features: local

fn serve(host: read String, max_connections: Int) -> Result<Unit, PoolError> {
    local pool_url = Url.from_string(value: read host)
    local pool = ResourcePool<DbConnection>.lazy(
        create: || DbConnection.open(url: read pool_url),
        max_size: max_connections,
    )
    with ResourcePool.try_borrow(pool: mut pool)? as conn {
        ResourcePool.discard(lease: mut conn)
    }
    let snapshot = ResourcePool.stats(pool: mut pool)
    let _ = PoolStats.in_use(stats: read snapshot)
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("pool.rss", source).expect("lazy pool source should lower");

    // Lazy factories are stored, so they own their captures via a `move` closure
    // and `max_size` may be a runtime value.
    assert!(
        rust.contains("let mut pool = rsscript_runtime::ResourcePool::lazy_from_factory(max_connections, move || rsscript_runtime::db_connection_open(&(pool_url)));"),
        "lazy factory should lower to a moving, runtime-sized factory, got:\n{rust}"
    );
    // Graceful checkout returns a `Result` propagated by `?`.
    assert!(
        rust.contains("rsscript_runtime::ResourcePool::try_borrow(&mut pool)"),
        "try_borrow should lower to the fallible checkout, got:\n{rust}"
    );
    assert!(
        rust.contains("rsscript_runtime::resource_lease_discard(&mut conn)"),
        "discard should lower to lease eviction, got:\n{rust}"
    );
    assert!(
        rust.contains("rsscript_runtime::pool_stats(&mut pool)"),
        "stats should lower to the pool stats hook, got:\n{rust}"
    );
}

#[test]
fn rust_lowering_maps_resource_pool_try_new_to_runtime_hooks() {
    let source = r#"
features: local

fn run_query(url: read Url, sql: read String) -> Result<Unit, DbError> {
    local pool = ResourcePool<DbConnection>.try_new(
        create: || DbConnection.try_open(url: read url),
        max_size: 2,
    )?

    with ResourcePool.borrow(pool: mut pool) as conn {
        DbConnection.query(conn: mut conn, sql: read sql)?
    }

    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("db-try-pool.rss", source).expect("source should lower");

    assert!(rust.contains("-> Result<(), rsscript_runtime::DbError>"));
    assert!(rust.contains("let mut pool = rsscript_runtime::ResourcePool::try_from_factory(2i64, || rsscript_runtime::db_connection_try_open(url))?;"));
    assert!(rust.contains("rsscript_runtime::db_connection_query(&mut conn, sql)?;"));
}

#[test]
fn checker_allows_arithmetic_with_typed_builtin_call_operands() {
    let source = r#"
fn main() -> Unit {
    let value = 20 + String.len(value: read "rss")
    return Unit
}
"#;
    let diagnostics = analyze_source_with_core("typed-arithmetic.rss", source);

    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code != "RS1001"),
        "{diagnostics:?}"
    );
}

#[test]
fn rust_lowering_maps_with_resource_drop_points() {
    let source = r#"
fn copy(path: read Path) -> Result<Unit, FileError> {
    with File.open_read(path: read path)? as file {
        File.read_all(file: mut file)?
    }

    return Ok(Unit)
}
"#;
    let lowered =
        lower_source_to_rust_with_map("resource.rss", source).expect("source should lower");

    assert!(
        lowered
            .rust_source
            .contains("    {\n        let mut file = rsscript_runtime::file_open_read(path)?;")
    );
    assert!(lowered.rust_source.contains(
        "        rsscript_runtime::file_read_all(&mut file)?;\n        // rss:span kind=resource_drop"
    ));
    assert!(
        lowered
            .rust_source
            .contains("// rss:span kind=resource_drop file=resource.rss")
    );
    assert!(
        lowered
            .source_map
            .iter()
            .any(|entry| entry.kind == "resource_drop" && entry.source.file == "resource.rss")
    );
}

#[test]
fn rust_lowering_emits_source_spans_for_resource_pool_borrow() {
    let source = r#"
features: local

resource TestConnection {
    fd: Int
}

fn TestConnection.query(conn: mut TestConnection, sql: read String) -> Unit {
    return Unit
}

fn pooled(pool: mut ResourcePool<TestConnection>) -> Unit {
    with ResourcePool.borrow(pool: mut pool) as conn {
        TestConnection.query(conn: mut conn, sql: read "select 1")
    }
}
"#;
    let rust = lower_source_to_rust("pool.rss", source).expect("source should lower");

    assert!(rust.contains(
        "let mut conn = rsscript_runtime::unwrap_runtime(rsscript_runtime::ResourcePool::borrow_at(pool, rsscript_runtime::SourceSpan::new(\"pool.rss\""
    ));
}

#[test]
fn checker_rejects_managed_type_drop_blocks() {
    let source = r#"
class Session {
    id: Int

    drop {
        Log.write(message: read "closing")
    }
}

struct BufferOwner {
    bytes: Bytes

    drop {
        Log.write(message: read "closing")
    }
}

resource File {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}
"#;
    let diagnostics = analyze_source("managed-drop.rss", source);
    let managed_drop_count = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "RS0015" && diagnostic.label == "unsupported managed drop"
        })
        .count();

    assert_eq!(managed_drop_count, 2, "{diagnostics:?}");
}

#[test]
fn checker_reports_malformed_with_statements_as_unsupported() {
    let source = r#"
fn missing_as(path: read Path) -> Unit {
    with File.open(path: read path) {
        return Unit
    }
}

fn missing_binding(path: read Path) -> Unit {
    with File.open(path: read path)? as {
        return Unit
    }
}
"#;
    let diagnostics = analyze_source("malformed-with.rss", source);
    let malformed_count = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "RS0015" && diagnostic.label == "malformed with statement"
        })
        .count();

    assert_eq!(malformed_count, 2, "{diagnostics:?}");
}

#[test]
fn parser_keeps_multiline_return_call_with_try_as_one_statement() {
    let source = r#"
fn make_response(
    status: read String,
    trace_id: read String,
) -> Result<fresh Response, HttpError> {
    return Response.ok(
        body: read String.concat(left: read status, right: read trace_id),
    )?
}
"#;
    let diagnostics = analyze_source("multiline-return-call.rss", source);
    assert_eq!(diagnostics, Vec::new(), "{diagnostics:?}");
}

#[test]
fn checker_rejects_resource_escape_through_effect_wrapper() {
    let source = r#"
features: local

resource File {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}

fn bad_return(path: read Path) -> File {
    with File.open(path: read path) as file {
        return read file
    }
}

fn bad_binding(path: read Path) -> Unit {
    with File.open(path: read path) as file {
        let saved = read file
    }
}
"#;
    let diagnostics = analyze_source("resource-effect-escape.rss", source);
    let escape_count = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "RS0702" && diagnostic.label == "resource escapes")
        .count();

    assert_eq!(escape_count, 2, "{diagnostics:?}");
}

#[test]
fn checker_rejects_resource_escape_through_result_wrapper() {
    let source = r#"
features: local

resource LogFile {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}

fn LogFile.open(path: read Path) -> LogFile

fn bad_return(path: read Path) -> Unit {
    with LogFile.open(path: read path) as file {
        return Ok(file)
    }
}

fn bad_binding(path: read Path) -> Unit {
    with LogFile.open(path: read path) as file {
        let saved = Some(file)
    }
}
"#;
    let diagnostics = analyze_source("resource-wrapper-escape.rss", source);
    let escape_count = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "RS0702" && diagnostic.label == "resource escapes")
        .count();

    assert_eq!(escape_count, 2, "{diagnostics:?}");
}

#[test]
fn checker_requires_resource_producers_to_enter_with_context() {
    let source = r#"
features: local

resource File {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}

fn File.open(path: read Path) -> File
fn File.open_result(path: read Path) -> Result<File, IOError>
fn File.stat(file: read File) -> Unit

fn ok_with(path: read Path) -> Result<Unit, IOError> {
    with File.open_result(path: read path)? as file {
        File.stat(file: read file)
    }
    return Ok(Unit)
}

fn bad_with_missing_try(path: read Path) -> Unit {
    with File.open_result(path: read path) as file {
        File.stat(file: read file)
    }
}

fn bad_let(path: read Path) -> Unit {
    let file = File.open(path: read path)
}

fn bad_result_let(path: read Path) -> Result<Unit, IOError> {
    let file = File.open_result(path: read path)?
    return Ok(Unit)
}

fn bad_return(path: read Path) -> File {
    return File.open(path: read path)
}

fn bad_result_return(path: read Path) -> Result<File, IOError> {
    return File.open_result(path: read path)?
}

fn bad_arg(path: read Path) -> Unit {
    File.stat(file: read File.open(path: read path))
}
"#;
    let diagnostics = analyze_source("resource-producer-context.rss", source);
    let producer_escape_count = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "RS0702" && diagnostic.label == "resource producer escapes"
        })
        .count();

    assert_eq!(producer_escape_count, 5, "{diagnostics:?}");
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0706" && diagnostic.label == "missing resource producer `?`"
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_allows_result_resource_only_as_return_producer_contract() {
    let source = r#"
resource File {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}

struct Holder {
    file: Result<File, IOError>
}

fn accept(result: read Result<File, IOError>) -> Unit
fn File.open(path: read Path) -> Result<File, IOError>
"#;
    let diagnostics = analyze_source("result-resource-contract.rss", source);
    let generic_resource_count = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "RS0704")
        .count();

    assert_eq!(generic_resource_count, 2, "{diagnostics:?}");
}

#[test]
fn checker_rejects_resource_pool_non_positive_or_dynamic_max_size() {
    let source = r#"
features: local

resource DbConnection {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}

fn DbConnection.open(url: read String) -> DbConnection

fn zero_pool(url: read String) -> Unit {
    local pool = ResourcePool<DbConnection>.new(
        create: || DbConnection.open(url: read url),
        max_size: 0,
    )
}

fn negative_pool(url: read String) -> Unit {
    local pool = ResourcePool<DbConnection>.new(
        create: || DbConnection.open(url: read url),
        max_size: -1,
    )
}

fn dynamic_pool(url: read String, count: Int) -> Unit {
    local pool = ResourcePool<DbConnection>.new(
        create: || DbConnection.open(url: read url),
        max_size: count,
    )
}
"#;
    let diagnostics = analyze_source("resourcepool-max-size.rss", source);
    let invalid_max_size_count = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "RS0708" && diagnostic.label == "invalid ResourcePool max_size"
        })
        .count();

    assert_eq!(invalid_max_size_count, 3, "{diagnostics:?}");
}

#[test]
fn checker_matches_resource_pool_constructor_to_factory_result_shape() {
    let source = r#"
features: local

resource DbConnection {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}

fn DbConnection.open(url: read String) -> DbConnection
fn DbConnection.try_open(url: read String) -> Result<DbConnection, DbError>

fn bad_new(url: read String) -> Unit {
    local pool = ResourcePool<DbConnection>.new(
        create: || DbConnection.try_open(url: read url),
        max_size: 1,
    )
}

fn bad_try_new(url: read String) -> Result<Unit, DbError> {
    local pool = ResourcePool<DbConnection>.try_new(
        create: || DbConnection.open(url: read url),
        max_size: 1,
    )?

    return Ok(Unit)
}
"#;
    let diagnostics = analyze_source("resourcepool-factory-shape.rss", source);
    let factory_contract_count = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "RS0707")
        .count();

    assert_eq!(factory_contract_count, 2, "{diagnostics:?}");
}

#[test]
fn checker_rejects_resource_pool_use_while_lease_is_active() {
    let source = r#"
features: local

resource DbConnection {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}

fn DbConnection.open(url: read String) -> DbConnection
fn Pool.consume(pool: take ResourcePool<DbConnection>) -> Unit
fn Pool.stats(pool: read ResourcePool<DbConnection>) -> Int

fn bad_nested_borrow(url: read String) -> Unit {
    local pool = ResourcePool<DbConnection>.new(
        create: || DbConnection.open(url: read url),
        max_size: 2,
    )

    with ResourcePool.borrow(pool: mut pool) as first {
        with ResourcePool.borrow(pool: mut pool) as second {
            Log.write(message: read "unreachable")
        }
    }
}

fn bad_take_during_lease(url: read String) -> Unit {
    local pool = ResourcePool<DbConnection>.new(
        create: || DbConnection.open(url: read url),
        max_size: 1,
    )

    with ResourcePool.borrow(pool: mut pool) as conn {
        Pool.consume(pool: take pool)
    }
}

fn bad_read_during_lease(url: read String) -> Unit {
    local pool = ResourcePool<DbConnection>.new(
        create: || DbConnection.open(url: read url),
        max_size: 1,
    )

    with ResourcePool.borrow(pool: mut pool) as conn {
        let count = Pool.stats(pool: read pool)
        Log.write(message: read "unreachable")
    }
}

fn ok_different_pool(url: read String) -> Unit {
    local first_pool = ResourcePool<DbConnection>.new(
        create: || DbConnection.open(url: read url),
        max_size: 1,
    )
    local second_pool = ResourcePool<DbConnection>.new(
        create: || DbConnection.open(url: read url),
        max_size: 1,
    )

    with ResourcePool.borrow(pool: mut first_pool) as conn {
        with ResourcePool.borrow(pool: mut second_pool) as other {
            Log.write(message: read "ok")
        }
    }
}
"#;
    let diagnostics = analyze_source("resourcepool-active-lease.rss", source);
    let active_lease_conflicts = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "RS0709" && diagnostic.label == "ResourcePool active lease conflict"
        })
        .count();

    assert_eq!(active_lease_conflicts, 3, "{diagnostics:?}");
}
