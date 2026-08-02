//! Spec §6.2/§7.5 — managed/handle/weak lowering
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn rust_lowering_wraps_managed_class_returns_in_managed_handle() {
    let source = r#"
class Session {
    id: Int
}

pub fn make_session(id: Int) -> Session {
    return Session(id: id)
}
"#;
    let rust = lower_source_to_rust("session.rss", source).expect("source should lower");

    assert!(rust.contains("pub struct Session"));
    assert!(rust.contains("pub fn make_session(id: i64) -> rsscript_runtime::Managed<Session>"));
    assert!(rust.contains("return rsscript_runtime::manage_at(Session { id: id }, rsscript_runtime::SourceSpan::new(\"session.rss\", 7, 12, 7));"));
}

#[test]
fn rust_lowering_treats_class_aliases_as_managed_everywhere() {
    let source = r#"
class Node {
    value: Int
}

type N = Node

struct Holder {
    node: N
}

fn make() -> N {
    return Node(value: 1)
}
"#;
    let rust = lower_source_to_rust("class-alias.rss", source).expect("source should lower");

    assert!(rust.contains("type N = rsscript_runtime::Managed<Node>;"));
    assert!(rust.contains("pub node: rsscript_runtime::Managed<Node>"));
    assert!(rust.contains("fn make() -> rsscript_runtime::Managed<Node>"));
    assert!(!rust.contains("Managed<rsscript_runtime::Managed<Node>>"));
}

#[test]
fn rust_lowering_preserves_generic_sums_and_managed_payloads() {
    let source = r#"
class Node {
    value: Int
}

pub sum Envelope<T> {
    Value(value: T)
    NodeValue(node: Node)
    Empty
}
"#;
    let rust = lower_source_to_rust("generic-sum.rss", source).expect("source should lower");

    assert!(rust.contains("pub enum Envelope<T: Clone>"));
    assert!(rust.contains("value: T"));
    assert!(rust.contains("node: rsscript_runtime::Managed<Node>"));
}

#[test]
fn rust_lowering_matches_user_sum_through_alias() {
    let source = r#"
sum Token {
    Number(value: Int)
    End
}

type Alias = Token

fn read_token(token: read Alias) -> Int {
    match read token {
        Number(value) => return value
        End => return 0
    }
}
"#;
    let rust = lower_source_to_rust("sum-alias-match.rss", source).expect("source should lower");

    assert!(rust.contains("Token::Number { value: value }"), "{rust}");
    assert!(rust.contains("Token::End"), "{rust}");
}

#[test]
fn rust_lowering_uses_managed_class_for_protocol_and_external_binding() {
    let source = r#"
protocol Readable {
    fn get(self: read Self) -> Int
}

class Gauge {
    value: Int
}

fn Gauge.get(self: read Gauge) -> Int {
    return self.value
}

impl Readable for Gauge {
    get = Gauge.get
}
"#;
    let rust = lower_source_to_rust("class-protocol.rss", source).expect("source should lower");

    assert!(
        rust.contains("impl Readable for rsscript_runtime::Managed<Gauge>"),
        "{rust}"
    );
    assert!(
        rust.contains("Gauge(rsscript_runtime::Managed<Gauge>)"),
        "{rust}"
    );
}

#[test]
fn rust_lowering_wraps_handle_fields_once() {
    let source = r#"
class User {
    id: Int
}

struct Session {
    owner: User
    explicit_owner: handle User
}
"#;
    let rust = lower_source_to_rust("session.rss", source).expect("source should lower");

    assert!(rust.contains("pub owner: rsscript_runtime::Managed<User>"));
    assert!(rust.contains("pub explicit_owner: rsscript_runtime::Managed<User>"));
    assert!(!rust.contains("rsscript_runtime::Managed<rsscript_runtime::Managed<User>>"));
}

#[test]
fn rust_lowering_wraps_and_reads_non_class_handle_fields() {
    let source = r#"
struct Boxed {
    value: Int
}

struct Holder {
    boxed: handle Boxed
}

fn read_boxed(boxed: read Boxed) -> Int {
    return boxed.value
}

fn make_holder() -> fresh Holder {
    let boxed = Boxed(value: 7)
    return Holder(boxed: read boxed)
}

fn call() -> Int {
    let holder = make_holder()
    return read_boxed(boxed: read holder.boxed)
}
"#;
    let rust = lower_source_to_rust("handle-boxed.rss", source).expect("source should lower");

    assert!(rust.contains("pub boxed: rsscript_runtime::Managed<Boxed>"));
    assert!(rust.contains("boxed: rsscript_runtime::manage_at(boxed.clone()"));
    assert!(
        rust.contains("read_boxed(&*rsscript_runtime::unwrap_runtime(holder.boxed.try_read_at(")
    );
}

#[test]
fn rust_lowering_maps_weak_upgrade_to_runtime_handle_upgrade() {
    let source = r#"
class User {
    id: Int
}

struct Session {
    owner: weak User
}

fn read_owner(session: read Session) -> Option<User> {
    return Weak.upgrade(value: read session.owner)
}
"#;
    let rust = lower_source_to_rust("weak-upgrade.rss", source).expect("source should lower");

    assert!(rust.contains("return session.owner.upgrade();"));
    assert!(!rust.contains("Weak_upgrade"));
}

#[test]
fn checker_requires_explicit_weak_handle_for_weak_field_initialization() {
    let source = r#"
class User {
    id: Int
}

struct Session {
    owner: weak User
}

fn make_session() -> Session {
    let user = User(id: 1)
    return Session(owner: read user)
}
"#;
    let diagnostics = analyze_source("weak-field-initializer.rss", source);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0904"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_requires_weak_field_upgrade_before_read_or_mut_use() {
    let source = r#"
class User {
    id: Int
}

struct Session {
    owner: weak User
}

fn User.log(user: read User) -> Unit
fn User.rename(user: mut User) -> Unit

fn bad_read(session: read Session) -> Unit {
    User.log(user: read session.owner)
}

fn bad_mut(session: read Session) -> Unit {
    User.rename(user: mut session.owner)
}
"#;
    let diagnostics = analyze_source("weak-field-direct-use.rss", source);
    let weak_use_count = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "RS0903")
        .count();

    assert_eq!(weak_use_count, 2, "{diagnostics:?}");
}

#[test]
fn checker_accepts_explicit_weak_upgrade() {
    let source = r#"
class User {
    id: Int
}

struct Session {
    owner: weak User
}

fn Weak.upgrade(value: read User) -> Option<User>

fn read_owner(session: read Session) -> Unit {
    let owner = Weak.upgrade(value: read session.owner)
}
"#;
    let diagnostics = analyze_source("weak-field-upgrade.rss", source);

    assert_eq!(diagnostics, Vec::new(), "{diagnostics:?}");
}

#[test]
fn rust_lowering_uses_shared_handles_for_managed_class_mut_parameters() {
    let source = r#"

class User {
    id: Int
}

fn touch(user: mut User) -> Unit {
    return Unit
}

fn call(user: mut User) -> Unit {
    touch(user: mut user)
}

fn promote(id: Int) -> Unit {
    let shared = User(id: id)
    touch(user: mut shared)
}
"#;
    let rust = lower_source_to_rust("user.rss", source).expect("source should lower");

    assert!(rust.contains("fn touch(user: &rsscript_runtime::Managed<User>)"));
    assert!(rust.contains("fn call(user: &rsscript_runtime::Managed<User>)"));
    assert!(rust.contains("touch(user);"));
    assert!(rust.contains("touch(&shared);"));
    assert!(!rust.contains("&mut rsscript_runtime::Managed<User>"));
    assert!(!rust.contains("touch(&mut shared);"));
}

#[test]
fn checker_rejects_effect_wrapped_managed_to_local() {
    let source = r#"

struct Widget

fn make_widget() -> fresh Widget

fn bad() -> Unit {
    let shared = make_widget()
    local read_copy = read shared
    local mut_copy = mut shared
    return Unit
}
"#;
    let diagnostics = analyze_source("managed-to-local-effect.rss", source);
    let managed_to_local_count = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "RS0301" && diagnostic.label == "managed value used as local"
        })
        .count();

    assert_eq!(managed_to_local_count, 2, "{diagnostics:?}");
}

#[test]
fn checker_rejects_wrapped_managed_bound_as_local() {
    let source = r#"

struct Widget

fn make_widget() -> fresh Widget

fn bad() -> Unit {
    let shared = make_widget()
    local maybe_shared = Some(shared)
    local read_maybe_shared = read Some(shared)
    return Unit
}
"#;
    let diagnostics = analyze_source("managed-wrapper-to-local.rss", source);
    let managed_to_local_count = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "RS0301" && diagnostic.label == "managed value used as local"
        })
        .count();

    assert_eq!(managed_to_local_count, 2, "{diagnostics:?}");
}

#[test]
fn checker_rejects_handle_field_bound_as_local() {
    let source = r#"

struct Rules
struct Holder {
    rules: handle Rules
}

fn make_holder() -> fresh Holder

fn bad() -> Unit {
    local holder = make_holder()
    local rules = holder.rules
    local read_rules = read holder.rules
    return Unit
}
"#;
    let diagnostics = analyze_source("handle-field-to-local.rss", source);
    let managed_to_local_count = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "RS0301" && diagnostic.label == "managed value used as local"
        })
        .count();

    assert_eq!(managed_to_local_count, 2, "{diagnostics:?}");
}

#[test]
fn checker_rejects_wrapped_handle_field_bound_as_local() {
    let source = r#"

struct Rules
struct Holder {
    rules: handle Rules
}

fn make_holder() -> fresh Holder

fn bad() -> Unit {
    local holder = make_holder()
    local maybe_rules = Some(holder.rules)
    local read_maybe_rules = read Some(holder.rules)
    return Unit
}
"#;
    let diagnostics = analyze_source("handle-field-wrapper-to-local.rss", source);
    let managed_to_local_count = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "RS0301" && diagnostic.label == "managed value used as local"
        })
        .count();

    assert_eq!(managed_to_local_count, 2, "{diagnostics:?}");
}

#[test]
fn checker_rejects_retaining_local_inline_field() {
    let source = r#"

struct Image
struct Holder {
    image: Image
}
struct Cache

fn Cache.store(cache: mut Cache, value: read Image) -> Unit

fn make_holder(path: read Path) -> fresh Holder

fn bad_store(cache: mut Cache, path: read Path) -> Unit {
    local holder = make_holder(path: read path)
    Cache.store(cache: mut cache, value: read holder.image)
}
"#;
    let diagnostics = analyze_source("retaining-local-field.rss", source);

    assert!(diagnostics.iter().any(
        |diagnostic| diagnostic.code == "RS0501" && diagnostic.label == "local value retained"
    ));
}

#[test]
fn checker_rejects_retaining_local_through_enum_wrapper() {
    let source = r#"

struct Image
struct Holder {
    image: Image
}
struct Cache

fn Cache.store_option(cache: mut Cache, value: read Option<Image>) -> Unit

fn Cache.store_result(cache: mut Cache, value: read Result<Image, StoreError>) -> Unit

fn make_holder() -> fresh Holder

fn bad_store(cache: mut Cache) -> Unit {
    local holder = make_holder()
    Cache.store_option(cache: mut cache, value: read Some(holder.image))
    Cache.store_result(cache: mut cache, value: read Ok(holder.image))
    Cache.store_option(cache: mut cache, value: read Some(read holder.image))
}
"#;
    let diagnostics = analyze_source("retaining-local-wrapper.rss", source);
    let retained_count = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "RS0501" && diagnostic.label == "local value retained"
        })
        .count();

    assert_eq!(retained_count, 3, "{diagnostics:?}");
}

#[test]
fn checker_accepts_managed_closure_capturing_handle_field() {
    let source = r#"

class Image

struct Holder {
    image: handle Image
}

fn make_holder(path: read Path) -> fresh Holder

fn use_image(image: read Image) -> Unit

fn ok_capture(path: read Path) -> Unit {
    local holder = make_holder(path: read path)
    let callback = || {
        use_image(image: read holder.image)
    }
}
"#;
    let diagnostics = analyze_source("managed-closure-handle-field.rss", source);

    assert_eq!(diagnostics, Vec::new());
}

#[test]
fn checker_rejects_same_call_managed_place_conflict() {
    let source = r#"
class Cache {
    count: Int
}

fn Cache.new() -> Cache
fn touch(first: mut Cache, second: read Cache) -> Unit

fn bad() -> Unit {
    let cache = Cache.new()
    touch(first: mut cache, second: read cache)
}
"#;
    let diagnostics = analyze_source("same-call-managed-place.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0303" && diagnostic.label == "field path conflict"
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_rejects_same_call_manage_conflict_independent_of_argument_order() {
    let source = r#"

struct Image {
    width: Int
}

fn Image.new() -> fresh Image
fn sink(left: read Image, right: read Image) -> Unit

fn bad_read_then_manage() -> Unit {
    local image = Image.new()
    sink(left: read image, right: read (manage image))
}

fn bad_manage_then_read() -> Unit {
    local image = Image.new()
    sink(left: read (manage image), right: read image)
}
"#;
    let diagnostics = analyze_source("same-call-manage-order.rss", source);
    let move_conflicts = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "RS0305")
        .count();

    assert_eq!(move_conflicts, 2, "{diagnostics:?}");
}

#[test]
fn review_reports_local_manage_boundary_changes() {
    let old_source = r#"

struct Image {
    pixels: Buffer
}

fn publish(path: read Path) -> Unit {
    Image.inspect(image: read Image.load(path: read path))
}
"#;
    let new_source = r#"

struct Image {
    pixels: Buffer
}

fn publish(path: read Path) -> Unit {
    local image = Image.load(path: read path)
    let shared = manage image
}
"#;

    let findings = review_sources("old.rss", old_source, "new.rss", new_source);
    let codes: Vec<String> = findings
        .iter()
        .map(|finding| finding.code.clone())
        .collect();

    assert!(codes.contains(&"RSR011".to_string()));
    let boundary = findings
        .iter()
        .find(|finding| finding.code == "RSR011")
        .expect("expected boundary review finding");
    assert!(boundary.summary.contains("added local binding `image`"));
    assert!(boundary.summary.contains("added manage `image`"));
    assert!(boundary.summary.contains("body[1]"));
    assert!(boundary.summary.contains("body[2].value"));
    assert_eq!(boundary.risk, ReviewRisk::Boundary);
    assert!(
        boundary
            .fixes
            .iter()
            .any(|fix| fix.kind == "review_local_manage_boundary")
    );
}

#[test]
fn review_map_marks_managed_closure_capture_retention() {
    let source = r#"
struct Image

fn retain_callback(image: read Image) -> Unit {
    let callback = || {
        let seen = image
    }
    return Unit
}
"#;
    let map = review_map_sources(vec![("managed-closure-retention.rss", source)]);
    let region = map
        .files
        .iter()
        .flat_map(|file| &file.regions)
        .find(|region| region.function == "retain_callback")
        .expect("retain_callback region should exist");

    assert_eq!(
        region.classification,
        ReviewMapClassification::ReviewRequired
    );
    assert!(
        region
            .reasons
            .iter()
            .any(|reason| reason == "managed closure retains `image`"),
        "{region:?}"
    );
    assert_eq!(map.summary.unknown.functions, 0);
}

#[test]
fn review_map_marks_inline_managed_closure_capture_retention_unless_noescape() {
    let source = r#"
struct Image
struct Callback

fn store(callback: read Callback) -> Unit

fn apply(callback: noescape Fn()) -> Unit {
    callback()
    return Unit
}

fn retain_inline(image: read Image) -> Unit {
    store(callback: read || {
        Image.inspect(image: read image)
    })
    return Unit
}

fn noescape_inline(image: read Image) -> Unit {
    apply(callback: || {
        Image.inspect(image: read image)
    })
    return Unit
}
"#;
    let map = review_map_sources(vec![("inline-closure-retention.rss", source)]);
    let retain_region = map
        .files
        .iter()
        .flat_map(|file| &file.regions)
        .find(|region| region.function == "retain_inline")
        .expect("retain_inline region should exist");
    let noescape_region = map
        .files
        .iter()
        .flat_map(|file| &file.regions)
        .find(|region| region.function == "noescape_inline")
        .expect("noescape_inline region should exist");
    let apply_region = map
        .files
        .iter()
        .flat_map(|file| &file.regions)
        .find(|region| region.function == "apply")
        .expect("apply region should exist");

    assert!(
        retain_region
            .reasons
            .iter()
            .any(|reason| reason == "managed closure retains `image`"),
        "{retain_region:?}"
    );
    assert!(
        !noescape_region
            .reasons
            .iter()
            .any(|reason| reason == "managed closure retains `image`"),
        "{noescape_region:?}"
    );
    assert!(
        apply_region
            .reasons
            .iter()
            .any(|reason| reason == "noescape callback call `callback`"),
        "{apply_region:?}"
    );
}
