//! Spec §3/§6.2 — register-VM execution: native host bindings and managed values
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn reg_vm_runs_weak_intrinsics_like_compiled_backend() {
    let source = r#"
features: local

class User {
    id: Int
}

struct Session {
    owner: weak User
}

fn main() -> Unit {
    let user = User(id: 7)
    let session = Session(owner: Weak.from(value: read user))
    let downgraded = Weak.downgrade(value: read user)
    match Weak.upgrade(value: read session.owner) {
        Some(owner) => Log.write(message: read String.from_int(value: owner.id))
        None => Log.write(message: read "missing")
    }
    match Weak.upgrade(value: read downgraded) {
        Some(owner) => Log.write(message: read String.from_int(value: owner.id))
        None => Log.write(message: read "missing")
    }
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-weak-intrinsics.rss", source, []);
}

#[test]
fn reg_vm_runs_native_host_bindings_like_interpreter() {
    fn host_open(args: Vec<NativeValue>) -> Result<NativeValue, String> {
        let [] = args.as_slice() else {
            return Err(format!("unexpected args: {args:?}"));
        };
        Ok(NativeValue::Native {
            type_name: "HostHandle".to_string(),
            id: 7,
        })
    }

    fn host_describe(args: Vec<NativeValue>) -> Result<NativeValue, String> {
        let [NativeValue::Native { type_name, id }] = args.as_slice() else {
            return Err(format!("unexpected args: {args:?}"));
        };
        Ok(NativeValue::String(format!("{type_name}:{id}")))
    }

    fn host_echo(args: Vec<NativeValue>) -> Result<NativeValue, String> {
        let [NativeValue::String(message)] = args.as_slice() else {
            return Err(format!("unexpected args: {args:?}"));
        };
        Ok(NativeValue::String(format!("host:{message}")))
    }

    let source = r#"
features: native

opaque struct HostHandle

native fn Host.open() -> HostHandle
    effects(native)

native fn Host.describe(handle: read HostHandle) -> String
    effects(native)

native fn Host.echo(message: read String) -> String
    effects(native)

fn main() -> Unit {
    let handle = Host.open()
    Log.write(message: read Host.describe(handle: read handle))
    Log.write(message: read Host.echo(message: read "native"))
    return Unit
}
"#;

    assert_reg_vm_with_native_output(
        "reg-vm-native-host.rss",
        source,
        [],
        [
            ("Host.open", host_open as NativeInterpreterFn),
            ("Host.describe", host_describe as NativeInterpreterFn),
            ("Host.echo", host_echo as NativeInterpreterFn),
        ],
        "HostHandle:7\nhost:native\n",
    );
}

#[test]
fn reg_vm_runs_receiver_native_host_bindings_like_interpreter() {
    fn alpha_open(args: Vec<NativeValue>) -> Result<NativeValue, String> {
        let [] = args.as_slice() else {
            return Err(format!("unexpected args: {args:?}"));
        };
        Ok(NativeValue::Native {
            type_name: "Alpha".to_string(),
            id: 1,
        })
    }

    fn beta_open(args: Vec<NativeValue>) -> Result<NativeValue, String> {
        let [] = args.as_slice() else {
            return Err(format!("unexpected args: {args:?}"));
        };
        Ok(NativeValue::Native {
            type_name: "Beta".to_string(),
            id: 2,
        })
    }

    fn alpha_describe(args: Vec<NativeValue>) -> Result<NativeValue, String> {
        let [NativeValue::Native { type_name, id }] = args.as_slice() else {
            return Err(format!("unexpected args: {args:?}"));
        };
        Ok(NativeValue::String(format!("alpha:{type_name}:{id}")))
    }

    fn beta_describe(args: Vec<NativeValue>) -> Result<NativeValue, String> {
        let [NativeValue::Native { type_name, id }] = args.as_slice() else {
            return Err(format!("unexpected args: {args:?}"));
        };
        Ok(NativeValue::String(format!("beta:{type_name}:{id}")))
    }

    let source = r#"
features: native

opaque struct Alpha
opaque struct Beta

native fn Alpha.open() -> Alpha
    effects(native)

native fn Alpha.describe(self: read Alpha) -> String
    effects(native)

native fn Beta.open() -> Beta
    effects(native)

native fn Beta.describe(self: read Beta) -> String
    effects(native)

fn main() -> Unit {
    let alpha = Alpha.open()
    let beta = Beta.open()
    Log.write(message: read alpha.describe())
    Log.write(message: read beta.describe())
    return Unit
}
"#;

    assert_reg_vm_with_native_output(
        "reg-vm-receiver-native-host.rss",
        source,
        [],
        [
            ("Alpha.open", alpha_open as NativeInterpreterFn),
            ("Alpha.describe", alpha_describe as NativeInterpreterFn),
            ("Beta.open", beta_open as NativeInterpreterFn),
            ("Beta.describe", beta_describe as NativeInterpreterFn),
        ],
        "alpha:Alpha:1\nbeta:Beta:2\n",
    );
}

#[test]
fn reg_vm_runs_resource_drop_cleanup_like_interpreter() {
    let source = r#"
features: local

resource FileHandle {
    fd: Int

    drop {
        Log.write(message: read "file-drop")
        OS.close(fd: fd)
    }
}

resource DbHandle {
    fd: Int

    drop {
        Log.write(message: read "db-drop")
        Db.close(fd: fd)
    }
}

fn main() -> Unit {
    with FileHandle(fd: 0) as file {
        Log.write(message: read "file-body")
        Log.write(message: read String.from_int(value: file.fd))
    }
    with DbHandle(fd: 0) as db {
        Log.write(message: read "db-body")
        Log.write(message: read String.from_int(value: db.fd))
    }
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-resource-drop-cleanup.rss", source, []);
}

#[test]
fn reg_vm_runs_resource_pool_try_new_like_backend() {
    let source = r#"
features: local native

fn main() -> Result<Unit, DbError> {
    let url = Url.from_string(value: read "db://vm-resource-pool")
    local eager = ResourcePool<DbConnection>.try_new(
        create: || {
            return DbConnection.try_open(url: read url)
        },
        max_size: 2,
    )?
    with ResourcePool.borrow(pool: mut eager) as session {
        DbConnection.query(conn: mut session, sql: read "select eager")?
    }

    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-resource-pool-try-new.rss", source, []);
}

#[test]
fn reg_vm_runs_resource_drop_unwind_like_interpreter() {
    let source = r#"
features: local

resource Handle {
    id: Int

    drop {
        Log.write(message: read "drop")
        Log.write(message: read String.from_int(value: id))
    }
}

fn return_case() -> Unit {
    with Handle(id: 1) as handle {
        Log.write(message: read "return-body")
        Log.write(message: read String.from_int(value: handle.id))
        return Unit
    }
    Log.write(message: read "return-after")
    return Unit
}

fn break_case() -> Unit {
    let mut index = 0
    while index < 1 {
        with Handle(id: 2) as handle {
            Log.write(message: read "break-body")
            Log.write(message: read String.from_int(value: handle.id))
            break
        }
        Log.write(message: read "break-after-with")
    }
    Log.write(message: read "after-break")
    return Unit
}

fn continue_case() -> Unit {
    let mut index = 0
    while index < 2 {
        with Handle(id: index + 3) as handle {
            Log.write(message: read "continue-body")
            Log.write(message: read String.from_int(value: handle.id))
            index = index + 1
            continue
        }
        Log.write(message: read "continue-after-with")
    }
    Log.write(message: read "after-continue")
    return Unit
}

fn fail() -> Result<Unit, String> {
    return Err(String.copy(value: read "boom"))
}

fn try_case() -> Result<Unit, String> {
    with Handle(id: 5) as handle {
        Log.write(message: read "try-body")
        Log.write(message: read String.from_int(value: handle.id))
        fail()?
    }
    Log.write(message: read "try-after")
    return Ok(Unit)
}

fn main() -> Unit {
    return_case()
    break_case()
    continue_case()
    match try_case() {
        Ok(_) => {
            Log.write(message: read "try-ok")
        }
        Err(message) => {
            Log.write(message: read message)
        }
    }
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-resource-drop-unwind.rss", source, []);
}
