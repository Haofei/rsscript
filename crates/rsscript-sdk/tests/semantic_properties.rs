//! Property coverage for ownership, retention, and resource escape invariants.

use proptest::prelude::*;
use rsscript_sdk::analyze_source;

fn has_code(source: &str, code: &str) -> bool {
    analyze_source("semantic-property.rss", source)
        .iter()
        .any(|diagnostic| diagnostic.code == code)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    #[test]
    fn retained_local_is_rejected_for_every_argument_effect(
        effect in prop::sample::select(vec!["read", "mut", "take"]),
        harmless_locals in 0usize..8,
    ) {
        let markers = (0..harmless_locals)
            .map(|index| format!("    let marker_{index} = {index}\n"))
            .collect::<String>();
        let source = format!(r#"
struct Widget {{ value: Int }}
fn store(value: {effect} Widget) -> Unit retains(value)
fn main() -> Unit {{
{markers}    local widget = Widget(value: 1)
    store(value: {effect} widget)
    return Unit
}}
"#);
        prop_assert!(has_code(&source, "RS0501"));
    }

    #[test]
    fn managed_value_cannot_be_used_after_ownership_transfer(
        reads_before_transfer in 0usize..6,
        reads_after_transfer in 1usize..6,
    ) {
        let before = "    inspect(value: read widget)\n".repeat(reads_before_transfer);
        let after = "    inspect(value: read widget)\n".repeat(reads_after_transfer);
        let source = format!(r#"
class Cache {{ entries: List<Widget> }}
struct Widget {{ value: Int }}
fn inspect(value: read Widget) -> Unit {{ return Unit }}
fn store(cache: mut Cache, value: Widget) -> Unit retains(value) {{ return Unit }}
fn main(cache: mut Cache) -> Unit {{
    local widget = Widget(value: 1)
{before}    store(cache: mut cache, value: (manage widget))
{after}    return Unit
}}
"#);
        prop_assert!(has_code(&source, "RS0401"));
    }

    #[test]
    fn with_scoped_resource_cannot_escape_through_retention(
        harmless_locals in 0usize..8,
    ) {
        let markers = (0..harmless_locals)
            .map(|index| format!("        let marker_{index} = {index}\n"))
            .collect::<String>();
        let source = format!(r#"
resource File {{
    fd: Int
    drop {{ OS.close(fd: fd) }}
}}
class Registry {{ entries: List<File> }}
fn register(registry: mut Registry, file: File) -> Unit retains(file) {{ return Unit }}
fn main(registry: mut Registry, path: Path) -> Result<Unit, IOError> {{
    with File.open(path)? as file {{
{markers}        register(registry: mut registry, file)
    }}
    return Ok(Unit)
}}
"#);
        prop_assert!(has_code(&source, "RS0702"));
    }
}
