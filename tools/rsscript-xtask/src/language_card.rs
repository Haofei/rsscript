//! Deterministic language-reference material generated from owning registries.

use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::Path;

use rsscript_diagnostics::diagnostic_explanations;
use rsscript_semantics::interface_catalog::{CORE_INTERFACES, STANDARD_PACKAGE_INTERFACES};
use rsscript_syntax::PARSER_KEYWORDS;
use rsscript_syntax::lexer::{BUILTIN_CONSTANTS, CONTEXTUAL_KEYWORDS, KEYWORDS};
use serde::Serialize;

const BEGIN: &str = "<!-- BEGIN GENERATED LANGUAGE CARD -->";
const END: &str = "<!-- END GENERATED LANGUAGE CARD -->";
const VERSION: u32 = 1;
const PARTIAL: &str = "partial";
const COMPLETE: &str = "complete";
const LANGUAGE_VERSION: &str = "0.7";
const SPEC_VERSION: &str = "RSScript_v0.7";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Keyword {
    word: String,
    category: String,
    contextual: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CatalogDiagnostic {
    code: String,
    title: String,
    explanation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CoreInterface {
    path: String,
    /// `core` for `CORE_INTERFACES`, `standard_package` for the
    /// `STANDARD_PACKAGE_INTERFACES` under `packages/`. Both are prelude-visible
    /// to a single-file check; only the first is available to a package build
    /// without an explicit dependency.
    kind: &'static str,
    sha256: String,
    source: String,
}

const CORE_KIND: &str = "core";
const STANDARD_PACKAGE_KIND: &str = "standard_package";

#[derive(Serialize)]
struct DiagnosticFixAvailability {
    scope: &'static str,
    available_from: [&'static str; 2],
    description: &'static str,
}

#[derive(Serialize)]
struct GrammarJson {
    schema: &'static str,
    version: u32,
    completeness: &'static str,
    provenance: [&'static str; 4],
    reserved_keywords: Vec<Keyword>,
    contextual_keywords: Vec<Keyword>,
    /// Words the parser matches positionally that the lexer leaves as plain
    /// identifiers. Omitting them made the published grammar surface claim a
    /// smaller language than the parser accepts.
    parser_keywords: Vec<Keyword>,
    builtin_constants: Vec<String>,
}

#[derive(Serialize)]
struct DiagnosticCatalogJson {
    schema: &'static str,
    version: u32,
    completeness: &'static str,
    provenance: [&'static str; 1],
    fixes: DiagnosticFixAvailability,
    diagnostics: Vec<CatalogDiagnostic>,
}

#[derive(Serialize)]
struct CoreInterfacesJson {
    schema: &'static str,
    version: u32,
    completeness: &'static str,
    provenance: [&'static str; 2],
    sha256_algorithm: &'static str,
    interfaces: Vec<CoreInterface>,
}

#[derive(Serialize)]
struct LanguageCardJson {
    schema: &'static str,
    version: u32,
    completeness: &'static str,
    provenance: [&'static str; 3],
    language_version: &'static str,
    specification_version: &'static str,
    grammar_sha256_algorithm: &'static str,
    grammar_sha256: String,
    reserved_keyword_count: usize,
    contextual_keyword_count: usize,
    parser_keyword_count: usize,
    builtin_constant_count: usize,
    diagnostic_count: usize,
    core_interface_count: usize,
    diagnostic_fixes: DiagnosticFixAvailability,
    canonical_call_example: String,
    /// The worked example for the forms that are a shape rather than a line:
    /// `protocol`/`impl`, `Dyn.from`, `let … else` and both closure spellings.
    canonical_forms_example: String,
    accepted_surface_sugar: Vec<SurfaceSugar>,
    canonical_surface_forms: Vec<CanonicalSurfaceForm>,
    signature_count: usize,
    signatures: Vec<InterfaceSignature>,
}

/// One surface form models get wrong, with the right and wrong spelling.
///
/// Every entry is a measured failure class from
/// `docs/planning/2026-09-model-failure-modes.md`; the card's single
/// `Namespace.function(...)` example eliminated Rust `::` path syntax outright
/// (6 candidates -> 0), which is the evidence that showing the form works.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CanonicalSurfaceForm {
    form: &'static str,
    right: &'static str,
    wrong: &'static str,
}

/// One `pub fn` signature from an interface source, as the formatter spells it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct InterfaceSignature {
    /// The declaring namespace (`Json`), or `null` for a free function.
    namespace: Option<String>,
    /// The declared name including its namespace (`Json.field_int`).
    name: String,
    /// The full one-line signature, parameter effects and return type included.
    signature: String,
    /// The interface file the declaration comes from.
    path: String,
    kind: &'static str,
}

fn canonical_surface_forms() -> Vec<CanonicalSurfaceForm> {
    vec![
        CanonicalSurfaceForm {
            form: "constructor call",
            right: "Report(title: take title, count: 0)",
            wrong: "Report { title: title, count: 0 }",
        },
        CanonicalSurfaceForm {
            form: "match arm",
            right: "Ok(value) => { return value }",
            wrong: "Ok(value) => value,",
        },
        CanonicalSurfaceForm {
            form: "`task_group`, `with` and `select` are statements",
            right: "task_group { spawn work() }",
            wrong: "let results = task_group { spawn work() }",
        },
        CanonicalSurfaceForm {
            form: "no tuple destructuring in `for`",
            right: "for key in Map.keys(map: counts) { }",
            wrong: "for (key, value) in counts { }",
        },
        CanonicalSurfaceForm {
            form: "mutable binding",
            right: "let mut total: Int = 0",
            wrong: "mut total: Int = 0",
        },
        // Measured as the class that survives the repair loop: 34 `RS0308`
        // instances across 17 candidate-appearances in the fresh samples, and
        // every one of them is `take` of a `let` binding or of a literal. The
        // card was complicit — it teaches that `take` is written explicitly at
        // the call site, and taught `let mut` as *the* binding form without
        // ever naming the binding form a `take` actually requires.
        CanonicalSurfaceForm {
            form: "a value you will `take` is bound with `local`",
            right: "local title = \"daily\"",
            wrong: "let title = \"daily\"",
        },
        CanonicalSurfaceForm {
            form: "`take` moves a binding, never a literal",
            right: "local title = \"daily\"; build(title: take title)",
            wrong: "build(title: take \"daily\")",
        },
        // Measured on 2026-09-19 as the largest class the card could name and
        // did not: `RS1001` appears in 26 of the 300 candidates and in 16 of
        // the 100 written *with* this card in the prompt, 28 instances in the
        // sonnet `language_card` set alone. Every instance is `+` (or `++`)
        // between two strings. The table already covers the other forms models
        // reach for from neighbouring languages; string concatenation was the
        // one it left silent, and silence is what the RS0206 and RS0308 rows
        // showed gets filled in with another language's spelling.
        CanonicalSurfaceForm {
            form: "strings are joined by a call, never by `+`",
            right: "String.concat(left: head, right: tail)",
            wrong: "head + tail",
        },
        // Measured on 2026-09-19: `fn(x: T) -> U { }` for `|x| { }` is the
        // *only* `RS0015` sub-class that survives sonnet's three-turn repair
        // loop — three candidates, all three of them this — and the three
        // tasks needing a closure are the largest part of the gap between the
        // old and new generation sets. The card contained no closure literal
        // anywhere.
        CanonicalSurfaceForm {
            form: "closure literal",
            right: "local double = |x| { return x * 2 }",
            wrong: "let double = fn(x: Int) -> Int { return x * 2 }",
        },
        // The two spellings are not interchangeable: `|x|` captures
        // implicitly, and a `captures(...)` list is written on the `fn` form.
        // Crossing them (`|x| captures(read base)`) is `RS0015`.
        CanonicalSurfaceForm {
            form: "closure with an explicit capture list",
            right: "local add = fn(x) captures(read base) { return x + base }",
            wrong: "local add = |x| captures(read base) { return x + base }",
        },
        // 12 candidates, 6 of them with the card: when a task needs `with`,
        // roughly half of them bind the resource with `=` instead of `as`.
        CanonicalSurfaceForm {
            form: "`with` binds its resource with `as`",
            right: "with File.open_read(path)? as file { }",
            wrong: "with file = File.open_read(path)? { }",
        },
        // Both protocol tasks fail for both models in every mode, ending on
        // `RS1301` after three repair turns. An `impl` block maps an existing
        // function into the protocol slot; it does not declare a method body,
        // which is what a model writes when it has only seen Rust or Swift.
        CanonicalSurfaceForm {
            form: "`impl` maps an existing function into a protocol slot",
            right: "impl Formatter for Point { format = Point.format }",
            wrong: "impl Formatter for Point { fn format(self: Point) -> fresh String { } }",
        },
        // The dynamic form is a call with both type arguments and a `take`,
        // not a constructor: `Dyn<P>(value)` is `RS0024` plus `RS0201`.
        CanonicalSurfaceForm {
            form: "dynamic dispatch is built by `Dyn.from`",
            right: "Dyn.from<Formatter, Point>(value: take point)",
            wrong: "Dyn<Formatter>(point)",
        },
        // `RS0020` never fired in 300 candidates, including in the task
        // written to require `let … else` — that task failed on `RS0206` and
        // `RS0203` instead, because the model reached for an `Option.unwrap`
        // that does not exist rather than for the form the language has.
        CanonicalSurfaceForm {
            form: "bind a pattern or leave the block",
            right: "let Some(inner) = value else { return \"none\" }",
            wrong: "let inner = Option.unwrap(value: value)",
        },
    ]
}

/// An alternate surface spelling the parser desugars to `canonical` before the
/// checker sees it. `rss fmt` rewrites `accepted` to `canonical`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct SurfaceSugar {
    accepted: &'static str,
    canonical: &'static str,
    desugars_in: &'static str,
}

fn accepted_surface_sugar() -> Vec<SurfaceSugar> {
    vec![
        SurfaceSugar {
            accepted: "T { field: value }",
            canonical: "T(field: value)",
            desugars_in: "parser",
        },
        SurfaceSugar {
            accepted: "Pattern => expr,",
            canonical: "Pattern => { expr }",
            desugars_in: "parser",
        },
    ]
}

pub fn run(root: &Path, check: bool) -> Result<(), Box<dyn Error>> {
    for (relative, contents) in generated_documents() {
        write_or_check(&root.join(relative), &contents, check)?;
    }
    let agent = root.join("AGENT.md");
    let existing = fs::read_to_string(&agent)?;
    let updated = update_machine_block(&existing, &agent_block());
    if check && existing != updated {
        return Err("AGENT.md generated language-card block is stale; run `cargo run -p rsscript-xtask -- language-card`".into());
    }
    if !check && existing != updated {
        fs::write(agent, updated)?;
    }
    println!(
        "language-card {}",
        if check { "is current" } else { "generated" }
    );
    Ok(())
}

fn generated_documents() -> BTreeMap<&'static str, String> {
    BTreeMap::from([
        ("docs/generated/grammar.md", grammar_document()),
        ("docs/generated/grammar.json", grammar_json()),
        ("docs/generated/keywords.md", keywords_document()),
        (
            "docs/generated/diagnostic-catalog.md",
            diagnostics_document(),
        ),
        ("docs/generated/diagnostic-catalog.json", diagnostics_json()),
        (
            "docs/generated/core-interfaces.md",
            core_interfaces_document(),
        ),
        (
            "docs/generated/core-interfaces.json",
            core_interfaces_json(),
        ),
        ("docs/generated/language-card.md", language_card_document()),
        ("docs/generated/language-card.json", language_card_json()),
        ("docs/generated/signatures.md", signatures_document()),
    ])
}

fn header(title: &str, source: &str) -> String {
    format!(
        "<!-- generated by `cargo run -p rsscript-xtask -- language-card`; do not edit -->\n\n# {title}\n\nSource: `{source}`.\n\n"
    )
}

fn grammar_document() -> String {
    let keywords = keywords();
    let mut output = header("RSScript grammar surface", "rsscript-syntax lexer tables");
    output.push_str("This is a generated lexical reference, not a replacement for the parser. The parser and tests remain authoritative for production grammar and recovery. Its machine-readable companion is [grammar.json](grammar.json), marked `partial` because it does not claim a full EBNF.\n\n## Reserved keyword classes\n\n");
    for category in categories() {
        output.push_str(&format!("### {category}\n\n"));
        for keyword in &keywords {
            if !keyword.contextual && keyword.category == category {
                output.push_str(&format!("- `{}`\n", keyword.word));
            }
        }
        output.push('\n');
    }
    output.push_str("## Contextual words\n\n");
    for keyword in keywords.iter().filter(|keyword| keyword.contextual) {
        output.push_str(&format!("- `{}` ({})\n", keyword.word, keyword.category));
    }
    output.push_str("\n## Parser-level words\n\nThe lexer leaves these as plain identifiers; the parser matches them as keywords in specific positions. They are declaration, clause, ownership and structured-concurrency words, and a program that uses one as an ordinary name will not parse where the parser expects the keyword.\n\n");
    for keyword in parser_keywords() {
        output.push_str(&format!("- `{}` ({})\n", keyword.word, keyword.category));
    }
    output.push_str("\n## Built-in constants and constructors\n\n");
    for constant in builtin_constants() {
        output.push_str(&format!("- `{constant}`\n"));
    }
    output
}

fn keywords_document() -> String {
    let mut output = header(
        "RSScript keyword classification",
        "rsscript-syntax::lexer::KEYWORDS",
    );
    output.push_str("Machine-readable lexical surface: [grammar.json](grammar.json).\n\n| Word | Classification | Reserved |\n| --- | --- | --- |\n");
    for keyword in keywords() {
        let reserved = if keyword.contextual {
            "contextual"
        } else {
            "yes"
        };
        output.push_str(&format!(
            "| `{}` | {} | {reserved} |\n",
            keyword.word, keyword.category
        ));
    }
    output
}

fn diagnostics_document() -> String {
    let mut output = header(
        "RSScript diagnostic catalog",
        "rsscript-diagnostics diagnostic registry",
    );
    output.push_str("Machine-readable catalog: [diagnostic-catalog.json](diagnostic-catalog.json). Static explanations do not invent fixes: machine-applicable edits are instance-level data returned by `rss check --json` or `rss fix --json`.\n\n| Code | Title | Explanation |\n| --- | --- | --- |\n");
    for diagnostic in diagnostics() {
        output.push_str(&format!(
            "| `{}` | {} | {} |\n",
            diagnostic.code,
            diagnostic.title,
            diagnostic.explanation.replace('|', "\\|")
        ));
    }
    output
}

fn core_interfaces_document() -> String {
    let core = core_interfaces();
    let standard_packages = standard_package_interfaces();
    let mut output = header(
        "RSScript core interfaces",
        "rsscript-semantics::interface_catalog::{CORE_INTERFACES, STANDARD_PACKAGE_INTERFACES}",
    );
    output.push_str(&format!("{} core interface files and {} standard-package interface files are prelude-visible to a single-file check. Machine-readable catalog: [core-interfaces.json](core-interfaces.json), where each entry carries a `kind` of `{CORE_KIND}` or `{STANDARD_PACKAGE_KIND}`.\n\n## Core interfaces\n\nPlatform-neutral, always available.\n\n", core.len(), standard_packages.len()));
    for interface in core {
        output.push_str(&format!("- `{}`\n", interface.path));
    }
    output.push_str("\n## Standard-package interfaces\n\nEqually prelude-visible to a single-file check or lowering. A package build must instead receive these through an explicit package dependency.\n\n");
    for interface in standard_packages {
        output.push_str(&format!("- `{}`\n", interface.path));
    }
    output
}

fn language_card_document() -> String {
    let keyword_data = keywords();
    let mut output = header(
        "RSScript language card",
        "syntax, diagnostics, and core interface registries",
    );
    output.push_str("A compact generated index for contributors and tools.\n\n");
    output.push_str(&format!("- {} reserved keywords, {} contextual words, {} parser-level words, and {} built-in constants.\n- {} documented diagnostic codes.\n- {} platform-neutral core interface files carrying {} callable signatures.\n\n", keyword_data.iter().filter(|keyword| !keyword.contextual).count(), keyword_data.iter().filter(|keyword| keyword.contextual).count(), parser_keywords().len(), builtin_constants().len(), diagnostics().len(), core_interfaces().len(), interface_signatures().len()));
    output.push_str("- [Grammar surface](grammar.md) ([JSON](grammar.json))\n- [Keyword classification](keywords.md)\n- [Diagnostic catalog](diagnostic-catalog.md) ([JSON](diagnostic-catalog.json))\n- [Core interfaces](core-interfaces.md) ([JSON](core-interfaces.json))\n- [Core interface signatures](signatures.md)\n\nMachine-readable summary: [language-card.json](language-card.json). Diagnostic explanation catalogs do not fabricate fixes; machine-applicable edits are instance-level data returned by `rss check --json` and `rss fix --json`.\n\n## Canonical call spelling\n\nNamed arguments stay named. A direct call-site `read` wrapper is omitted because it is the default; `mut` and `take` remain explicit. The formatter does not invent or remove argument labels. `take` moves the value out of its binding, so its operand must be a `local` binding — a `let` binding and a literal cannot be taken, and a parameter that wants `take` therefore needs a `local` on the caller's side.\n\n```rsscript\n");
    output.push_str(&canonical_example());
    output.push_str("```\n\n");
    output.push_str(ACCEPTED_SUGAR_SECTION);
    output.push_str(&canonical_surface_forms_section());
    output.push_str(&core_signatures_section());
    output
}

/// The forms models get wrong most often, each with a right/wrong pair.
///
/// The card used to be silent on exactly these, and hallucinated syntax stayed
/// the largest first-attempt failure class (19 candidates without the card, 16
/// with it). Silence is what costs; the one call-spelling example the card
/// already carried removed Rust `::` paths entirely. Every row added since has
/// behaved the same way: the construct the row names disappears from the next
/// draw.
fn canonical_surface_forms_section() -> String {
    let mut output = String::from(
        "## Canonical surface forms\n\nThese are the forms most often written wrong. The right column is what `rss fmt` prints.\n\n| Form | Write this | Not this |\n| --- | --- | --- |\n",
    );
    for form in canonical_surface_forms() {
        output.push_str(&format!(
            "| {} | `{}` | `{}` |\n",
            form.form,
            table_cell(form.right),
            table_cell(form.wrong)
        ));
    }
    output.push_str(CANONICAL_FORMS_EXAMPLE_INTRO);
    output.push_str(&canonical_forms_example());
    output.push_str("```\n\n");
    output
}

/// A table cell's contents, with the column separator escaped.
///
/// A closure literal is spelled with the same character Markdown uses to end a
/// cell, so the row that finally shows `|x| { }` would have silently shredded
/// the table that shows it.
fn table_cell(value: &str) -> String {
    value.replace('|', "\\|")
}

const CANONICAL_FORMS_EXAMPLE_INTRO: &str = r#"
Four of those rows are a shape rather than a line. This program is the whole
shape, exactly as `rss fmt` prints it and exactly as `rss check` accepts it: a
`protocol` with an `impl` that maps an existing function into its slot, dynamic
dispatch built by `Dyn.from`, a `let ... else` that leaves the block, and both
closure spellings — `|x|` captures implicitly, `fn(x) captures(...)` lists what
it captures.

```rsscript
"#;

/// The whole callable surface, inline in the card.
///
/// The measurement is blunt about why this exists: `RS0206` is the largest
/// class that survives everything, and the only class that is large in *both*
/// measured models — 28 of 50 sonnet candidates and 31 of 50 haiku candidates
/// written *with* this card in the prompt. The card named 35 interface *files*
/// and linked a 450-signature index it never showed, so a model reading only
/// the card saw not one signature. Models filled that vacuum with `print`,
/// `Int.parse` and a nine-call `get_*` JSON accessor family that does not
/// exist.
///
/// Pointing at an index does not make a model read it, so the index is no
/// longer pointed at: it is here, complete and untruncated, in the same
/// generated form [`signatures_document`] renders. That costs roughly 760
/// lines, which is the acceptable price of a reference read once per prompt.
fn core_signatures_section() -> String {
    let signatures = interface_signatures();
    let namespaces = namespace_count(&signatures);
    let mut output = format!(
        "## Core interface signatures\n\n{} callable signatures across {namespaces} namespaces are prelude-visible to a single-file check. Every one of them is listed below, grouped by namespace and generated from the interface sources themselves; nothing is truncated and nothing outside this list is callable without declaring it. The same list stands alone as [signatures.md](signatures.md), and its machine-readable form is the `signatures` array of [language-card.json](language-card.json).\n\nA call is `Namespace.function(label: value)`. Parameter declarations keep `read`; at a call site `read` is omitted because it is the default, while `mut` and `take` stay explicit.\n\nAt a cursor, `rss generate continuations` returns the signatures for the namespace being typed, and every callable candidate carries its parameters as data: the label to write, whether that label may be dropped, and the effect the call site has to supply. Argument labels are the callee's own names, not the caller's: `String.split` takes `value` and `delimiter`, not `text` and `separator`. Read them rather than guessing them.\n",
        signatures.len()
    );
    output.push_str(&grouped_signature_list(&signatures, "###"));
    output
}

/// Every signature grouped by namespace and, within a namespace, by interface
/// file. `heading` is the namespace heading level: `##` standalone, `###` when
/// nested under the card's own section.
///
/// One renderer serves both so the card and the standalone index cannot drift
/// into disagreeing about what exists.
fn grouped_signature_list(signatures: &[InterfaceSignature], heading: &str) -> String {
    let mut output = String::new();
    let mut namespace = None;
    let mut path = None;
    for signature in signatures {
        if namespace.as_ref() != Some(&signature.namespace) {
            namespace = Some(signature.namespace.clone());
            path = None;
            output.push_str(&format!(
                "\n{heading} {}\n",
                signature
                    .namespace
                    .as_deref()
                    .unwrap_or("Free functions (no namespace)")
            ));
        }
        if path.as_deref() != Some(signature.path.as_str()) {
            path = Some(signature.path.clone());
            output.push_str(&format!("\n`{}`\n\n", signature.path));
        }
        output.push_str(&format!("- `{}`\n", signature.signature));
    }
    output
}

fn namespace_count(signatures: &[InterfaceSignature]) -> usize {
    let mut namespaces = signatures
        .iter()
        .filter_map(|signature| signature.namespace.clone())
        .collect::<Vec<_>>();
    namespaces.sort();
    namespaces.dedup();
    namespaces.len()
}

/// Every prelude-visible `pub fn`, grouped by namespace.
///
/// The list is deliberately never truncated: a signature index that silently
/// drops entries is worse than none, because a caller cannot tell absence from
/// omission. It is grouped by namespace and, within a namespace, by interface
/// file, so its size stays navigable as it grows.
fn signatures_document() -> String {
    let signatures = interface_signatures();
    let mut output = header(
        "RSScript core interface signatures",
        "rsscript-semantics::interface_catalog::{CORE_INTERFACES, STANDARD_PACKAGE_INTERFACES}",
    );
    output.push_str(&format!(
        "Every public function a single-file check can call without declaring anything — top-level `pub fn` and `pub async fn` declarations, plus the methods declared by a `protocol` — as {} signatures across {} namespaces, spelled exactly as `rss fmt` prints them. Nothing here is truncated. The machine-readable form is the `signatures` array of [language-card.json](language-card.json).\n\nA call is `Namespace.function(label: value)`. Parameter declarations keep `read`; at a call site `read` is omitted because it is the default, while `mut` and `take` stay explicit.\n",
        signatures.len(),
        namespace_count(&signatures)
    ));
    output.push_str(&grouped_signature_list(&signatures, "##"));
    output
}

/// Parse every prelude interface source and render each public declaration
/// through the formatter, so the card cannot disagree with the compiler about
/// what exists or how it is spelled.
fn interface_signatures() -> Vec<InterfaceSignature> {
    let mut signatures = Vec::new();
    for interface in prelude_interfaces() {
        let program = rsscript_syntax::parse_source_raw(&interface.path, &interface.source);
        for item in &program.items {
            let rsscript_syntax::ast::Item::Function(function) = item else {
                continue;
            };
            if !function.is_public {
                continue;
            }
            let (namespace, _) = function
                .name
                .rsplit_once('.')
                .map_or((None, function.name.as_str()), |(namespace, name)| {
                    (Some(namespace.to_string()), name)
                });
            signatures.push(InterfaceSignature {
                namespace,
                name: function.name.clone(),
                signature: rsscript_syntax::format_declaration_signature(function),
                path: interface.path.clone(),
                kind: interface.kind,
            });
        }
    }
    signatures.sort_by(|left, right| {
        left.namespace
            .cmp(&right.namespace)
            .then(left.path.cmp(&right.path))
            .then(left.signature.cmp(&right.signature))
    });
    signatures
}

/// Two surface spellings models already write are accepted and desugared by the
/// parser. Both are listed here because silence about them was measured to cost
/// more than the sugar does: see `docs/planning/2026-09-model-failure-modes.md`.
const ACCEPTED_SUGAR_SECTION: &str = r#"## Accepted surface sugar

Two alternate spellings are accepted and desugared by the parser to the
canonical form. They produce the same AST, so the checker sees only the
canonical node, and `rss fmt` rewrites them to the canonical spelling —
formatting is the normalizer.

| Also accepted | Canonical |
| --- | --- |
| `T { field: value }` | `T(field: value)` |
| `Pattern => expr,` | `Pattern => { expr }` |

A trailing comma after a block arm (`Pattern => { ... },`) is accepted too.
Prefer the canonical spelling when writing new code.

"#;

/// A second, larger canonical example: the four forms that are a shape rather
/// than a line.
///
/// Verified twice by
/// [`tests::canonical_forms_example_is_valid_and_already_formatted`] — it
/// checks clean and `rss fmt` is a fixpoint on it — so the card can never show
/// a protocol, a `Dyn.from`, a `let … else` or a closure the compiler would
/// reject. `with ... as` is the one row without a line here: a resource
/// producer must be declared bodyless in an `.rssi`, so no self-contained
/// program can demonstrate it; the SDK fixture
/// `multiline-with-as-resource.rss` carries that proof instead.
fn canonical_forms_example() -> String {
    r#"protocol Formatter {
    fn format(self: Self) -> fresh String
}

struct Point {
    x: Int
    y: Int
}

fn Point.format(self: Point) -> fresh String {
    return String.from_int(value: self.x)
}

fn render(x: Int, y: Int) -> fresh String {
    local point = Point(x: x, y: y)
    let shape = Dyn.from<Formatter, Point>(value: take point)
    return Formatter.format(self: shape)
}

fn shifted(value: Option<Int>) -> Int {
    let Some(base) = value else {
        return 0
    }
    local add = |x| {
        return x + base
    }
    local scale = fn(x) captures(read base) {
        return x * base
    }
    return add(2) + scale(3)
}

impl Formatter for Point {
    format = Point.format
}
"#
    .to_string()
}

fn canonical_example() -> String {
    "fn apply(target: mut Buffer, input: take String, note: read String) -> Unit {}\n\nfn update(target: mut Buffer, input: take String, note: read String) -> Unit {\n    return apply(target: mut target, input: take input, note: note)\n}\n".to_string()
}

fn grammar_json() -> String {
    let all_keywords = keywords();
    json(&GrammarJson {
        schema: "rsscript.grammar.v1",
        version: VERSION,
        completeness: PARTIAL,
        provenance: [
            "rsscript_syntax::lexer::KEYWORDS",
            "rsscript_syntax::lexer::CONTEXTUAL_KEYWORDS",
            "rsscript_syntax::PARSER_KEYWORDS",
            "rsscript_syntax::lexer::BUILTIN_CONSTANTS",
        ],
        reserved_keywords: all_keywords
            .iter()
            .filter(|keyword| !keyword.contextual)
            .cloned()
            .collect(),
        contextual_keywords: all_keywords
            .into_iter()
            .filter(|keyword| keyword.contextual)
            .collect(),
        parser_keywords: parser_keywords(),
        builtin_constants: builtin_constants(),
    })
}

fn diagnostics_json() -> String {
    json(&DiagnosticCatalogJson {
        schema: "rsscript.diagnostic-catalog.v1",
        version: VERSION,
        completeness: COMPLETE,
        provenance: ["rsscript_diagnostics::diagnostic_explanations"],
        fixes: diagnostic_fix_availability(),
        diagnostics: diagnostics(),
    })
}

fn core_interfaces_json() -> String {
    json(&CoreInterfacesJson {
        schema: "rsscript.core-interfaces.v1",
        version: VERSION,
        completeness: COMPLETE,
        provenance: [
            "rsscript_semantics::interface_catalog::CORE_INTERFACES",
            "rsscript_semantics::interface_catalog::STANDARD_PACKAGE_INTERFACES",
        ],
        sha256_algorithm: "sha256",
        interfaces: prelude_interfaces(),
    })
}

fn language_card_json() -> String {
    let keyword_data = keywords();
    json(&LanguageCardJson {
        schema: "rsscript.language-card.v1",
        version: VERSION,
        completeness: PARTIAL,
        provenance: [
            "rsscript_syntax::lexer",
            "rsscript_diagnostics::diagnostic_explanations",
            "rsscript_semantics::interface_catalog::CORE_INTERFACES",
        ],
        language_version: LANGUAGE_VERSION,
        specification_version: SPEC_VERSION,
        grammar_sha256_algorithm: "sha256",
        grammar_sha256: grammar_sha256(),
        reserved_keyword_count: keyword_data
            .iter()
            .filter(|keyword| !keyword.contextual)
            .count(),
        contextual_keyword_count: keyword_data
            .iter()
            .filter(|keyword| keyword.contextual)
            .count(),
        parser_keyword_count: parser_keywords().len(),
        builtin_constant_count: builtin_constants().len(),
        diagnostic_count: diagnostics().len(),
        core_interface_count: core_interfaces().len(),
        diagnostic_fixes: diagnostic_fix_availability(),
        canonical_call_example: canonical_example(),
        canonical_forms_example: canonical_forms_example(),
        accepted_surface_sugar: accepted_surface_sugar(),
        canonical_surface_forms: canonical_surface_forms(),
        signature_count: interface_signatures().len(),
        signatures: interface_signatures(),
    })
}

fn json(value: &impl Serialize) -> String {
    let mut output = serde_json::to_string_pretty(value).expect("generated JSON is serializable");
    output.push('\n');
    output
}

fn grammar_sha256() -> String {
    sha256(&grammar_json())
}

fn sha256(source: &str) -> String {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(source.as_bytes());
    format!("{digest:x}")
}

fn diagnostic_fix_availability() -> DiagnosticFixAvailability {
    DiagnosticFixAvailability {
        scope: "instance_only",
        available_from: ["rss check --json", "rss fix --json"],
        description: "Static diagnostic explanations do not claim fixes; machine-applicable edits are attached to concrete diagnostic instances.",
    }
}

fn categories() -> [&'static str; 4] {
    ["control", "declaration", "modifier", "ownership"]
}

fn keywords() -> Vec<Keyword> {
    let mut output = KEYWORDS
        .iter()
        .map(|(word, category)| Keyword {
            word: (*word).to_string(),
            category: format!("{category:?}").to_ascii_lowercase(),
            contextual: false,
        })
        .collect::<Vec<_>>();
    output.extend(CONTEXTUAL_KEYWORDS.iter().map(|(word, category)| Keyword {
        word: (*word).to_string(),
        category: format!("{category:?}").to_ascii_lowercase(),
        contextual: true,
    }));
    output.sort_by(|left, right| left.word.cmp(&right.word));
    output
}

/// The parser's own positional-keyword table, rendered like the lexer tables.
///
/// `docs/generated/grammar.md` omitted every one of these, so the published
/// grammar surface described a smaller language than the parser accepts.
/// `rsscript_syntax` owns the table and tests it against its own productions;
/// this generator only renders it.
fn parser_keywords() -> Vec<Keyword> {
    let mut keywords = PARSER_KEYWORDS
        .iter()
        .map(|(word, category)| Keyword {
            word: (*word).to_string(),
            category: format!("{category:?}").to_ascii_lowercase(),
            contextual: true,
        })
        .collect::<Vec<_>>();
    keywords.sort_by(|left, right| left.word.cmp(&right.word));
    keywords
}

fn builtin_constants() -> Vec<String> {
    let mut constants = BUILTIN_CONSTANTS
        .iter()
        .map(|constant| (*constant).to_string())
        .collect::<Vec<_>>();
    constants.sort();
    constants
}

fn diagnostics() -> Vec<CatalogDiagnostic> {
    let mut catalog = diagnostic_explanations()
        .iter()
        .map(|diagnostic| CatalogDiagnostic {
            code: diagnostic.code.to_string(),
            title: diagnostic.title.to_string(),
            explanation: diagnostic.explanation.to_string(),
        })
        .collect::<Vec<_>>();
    catalog.sort_by(|left, right| left.code.cmp(&right.code));
    catalog
}

fn core_interfaces() -> Vec<CoreInterface> {
    catalog_interfaces(CORE_INTERFACES, CORE_KIND)
}

fn standard_package_interfaces() -> Vec<CoreInterface> {
    catalog_interfaces(STANDARD_PACKAGE_INTERFACES, STANDARD_PACKAGE_KIND)
}

/// Every interface a single-file check sees without declaring anything, in one
/// path-ordered list; the `kind` field keeps the two registries distinguishable.
fn prelude_interfaces() -> Vec<CoreInterface> {
    let mut interfaces = core_interfaces();
    interfaces.extend(standard_package_interfaces());
    interfaces.sort_by(|left, right| left.path.cmp(&right.path));
    interfaces
}

fn catalog_interfaces(entries: &[(&str, &str)], kind: &'static str) -> Vec<CoreInterface> {
    let mut interfaces = entries
        .iter()
        .map(|(path, source)| CoreInterface {
            path: (*path).to_string(),
            kind,
            sha256: sha256(source),
            source: (*source).to_string(),
        })
        .collect::<Vec<_>>();
    interfaces.sort_by(|left, right| left.path.cmp(&right.path));
    interfaces
}

fn agent_block() -> String {
    format!(
        "{BEGIN}\n\n## Generated language card\n\nThe generated [language card](docs/generated/language-card.md) and machine-readable [language card](docs/generated/language-card.json), [grammar](docs/generated/grammar.json), [diagnostic catalog](docs/generated/diagnostic-catalog.json), and [core-interface catalog](docs/generated/core-interfaces.json) are derived from syntax keyword tables, the diagnostic registry, and the core-interface catalog. Refresh them with `cargo run -p rsscript-xtask -- language-card`; verify freshness with the same command plus `--check`.\n\n{END}"
    )
}

fn update_machine_block(document: &str, block: &str) -> String {
    match (document.find(BEGIN), document.find(END)) {
        (Some(start), Some(end)) if start < end => format!(
            "{}{}{}",
            &document[..start],
            block,
            &document[end + END.len()..]
        ),
        _ => format!("{}\n\n{}\n", document.trim_end(), block),
    }
}

fn write_or_check(path: &Path, contents: &str, check: bool) -> Result<(), Box<dyn Error>> {
    if check {
        if fs::read_to_string(path).unwrap_or_default() != contents {
            return Err(format!(
                "{} is stale; run `cargo run -p rsscript-xtask -- language-card`",
                path.display()
            )
            .into());
        }
    } else {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        if fs::read_to_string(path).ok().as_deref() != Some(contents) {
            fs::write(path, contents)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_block_is_replaced_without_touching_surrounding_text() {
        let input = format!("before\n{BEGIN}\nold\n{END}\nafter\n");
        assert_eq!(update_machine_block(&input, "new"), "before\nnew\nafter\n");
    }

    #[test]
    fn registry_sources_produce_the_canonical_card() {
        assert!(keywords().iter().any(|keyword| keyword.word == "take"));
        assert!(
            diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == "RS0202")
        );
        assert!(
            core_interfaces()
                .iter()
                .any(|interface| interface.path.ends_with("string.rssi"))
        );
        // Standard-package interfaces are prelude-visible too, so the catalog
        // lists them beside the core ones, distinguished by `kind`.
        assert!(prelude_interfaces().iter().any(|interface| interface.path
            == "packages/async/interface/task.rssi"
            && interface.kind == STANDARD_PACKAGE_KIND));
        assert!(prelude_interfaces().iter().any(
            |interface| interface.path.ends_with("string.rssi") && interface.kind == CORE_KIND
        ));
        assert_eq!(
            prelude_interfaces().len(),
            core_interfaces().len() + standard_package_interfaces().len()
        );
        assert!(canonical_example().contains("note: note"));
    }

    #[test]
    fn machine_readable_catalogs_have_stable_provenance_and_declared_scope() {
        let grammar: serde_json::Value = serde_json::from_str(&grammar_json()).unwrap();
        assert_eq!(grammar["schema"], "rsscript.grammar.v1");
        assert_eq!(grammar["version"], VERSION);
        assert_eq!(grammar["completeness"], PARTIAL);
        assert!(grammar["reserved_keywords"].is_array());

        let diagnostics: serde_json::Value = serde_json::from_str(&diagnostics_json()).unwrap();
        assert_eq!(
            diagnostics["provenance"][0],
            "rsscript_diagnostics::diagnostic_explanations"
        );
        assert_eq!(diagnostics["completeness"], COMPLETE);
        assert_eq!(diagnostics["fixes"]["scope"], "instance_only");
        assert_eq!(
            diagnostics["fixes"]["available_from"][0],
            "rss check --json"
        );
        let diagnostic_codes = diagnostics["diagnostics"]
            .as_array()
            .expect("diagnostics array")
            .iter()
            .map(|diagnostic| diagnostic["code"].as_str().expect("diagnostic code"))
            .collect::<Vec<_>>();
        assert!(diagnostic_codes.windows(2).all(|pair| pair[0] <= pair[1]));

        let interfaces: serde_json::Value = serde_json::from_str(&core_interfaces_json()).unwrap();
        assert_eq!(interfaces["completeness"], COMPLETE);
        assert_eq!(interfaces["sha256_algorithm"], "sha256");
        let interface_paths = interfaces["interfaces"]
            .as_array()
            .expect("interfaces array")
            .iter()
            .map(|interface| interface["path"].as_str().expect("interface path"))
            .collect::<Vec<_>>();
        assert!(interface_paths.windows(2).all(|pair| pair[0] <= pair[1]));
        let string = interfaces["interfaces"]
            .as_array()
            .and_then(|catalog| {
                catalog
                    .iter()
                    .find(|interface| interface["path"] == "stdlib/string/string.rssi")
            })
            .expect("string interface");
        let source = string["source"].as_str().expect("string source");
        assert!(source.contains("pub fn String.concat"));
        assert_eq!(string["sha256"], sha256(source));

        let card: serde_json::Value = serde_json::from_str(&language_card_json()).unwrap();
        assert_eq!(card["schema"], "rsscript.language-card.v1");
        assert_eq!(card["language_version"], LANGUAGE_VERSION);
        assert_eq!(card["specification_version"], SPEC_VERSION);
        assert_eq!(card["grammar_sha256_algorithm"], "sha256");
        assert_eq!(card["grammar_sha256"], grammar_sha256());
        assert_eq!(card["diagnostic_fixes"]["scope"], "instance_only");
        assert!(
            card["canonical_call_example"]
                .as_str()
                .is_some_and(|example| example.contains("note: note"))
        );
    }

    /// The card may only advertise sugar the parser really accepts, and it must
    /// name the spelling `rss fmt` actually prints.
    #[test]
    fn advertised_surface_sugar_formats_to_its_canonical_spelling() {
        let sugared = "fn build(title: take String) -> Report {\n    return Report { title: take title }\n}\n\nfn classify(value: Int) -> Int {\n    return match value {\n        0 => 10,\n        _ => 20,\n    }\n}\n";
        let formatted = rsscript_syntax::format_source("card.rss", sugared);
        assert!(
            formatted.contains("Report(title: take title)"),
            "brace struct literal must format to the constructor call: {formatted}"
        );
        assert!(
            !formatted.contains("=> 10,"),
            "expression arm must format to a block arm: {formatted}"
        );

        for sugar in accepted_surface_sugar() {
            assert!(
                ACCEPTED_SUGAR_SECTION.contains(sugar.accepted)
                    && ACCEPTED_SUGAR_SECTION.contains(sugar.canonical),
                "the rendered card must document `{}` -> `{}`",
                sugar.accepted,
                sugar.canonical
            );
        }
    }

    /// The published grammar surface must list every word the parser treats as
    /// a keyword, not only the ones the lexer reserves. It used to omit all
    /// seventeen parser-level words, so it described a smaller language than
    /// the parser accepts.
    #[test]
    fn grammar_surface_lists_the_parser_level_words() {
        let document = grammar_document();
        let grammar: serde_json::Value = serde_json::from_str(&grammar_json()).unwrap();

        assert_eq!(
            grammar["parser_keywords"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            rsscript_syntax::PARSER_KEYWORDS.len()
        );
        assert!(
            grammar["provenance"]
                .as_array()
                .expect("provenance array")
                .iter()
                .any(|entry| entry == "rsscript_syntax::PARSER_KEYWORDS"),
            "the parser table must be named as a source of the grammar surface"
        );

        for (word, _) in rsscript_syntax::PARSER_KEYWORDS {
            assert!(
                document.contains(&format!("- `{word}` (")),
                "`{word}` is missing from the rendered grammar surface"
            );
        }
        for word in [
            "sum",
            "protocol",
            "impl",
            "type",
            "const",
            "opaque",
            "derives",
            "retains",
            "noescape",
            "owned",
            "captures",
            "task_group",
            "select",
            "spawn",
            "use",
            "module",
        ] {
            assert!(
                rsscript_syntax::PARSER_KEYWORDS
                    .iter()
                    .any(|(published, _)| *published == word),
                "`{word}` must be published as a parser keyword"
            );
        }

        let card: serde_json::Value = serde_json::from_str(&language_card_json()).unwrap();
        assert_eq!(
            card["parser_keyword_count"].as_u64().unwrap_or_default() as usize,
            rsscript_syntax::PARSER_KEYWORDS.len()
        );
    }

    /// `RS0207` is a semantic-frontend check, not a Rust-backend one.
    #[test]
    fn argument_type_mismatch_does_not_name_an_archived_backend() {
        let explanation = diagnostics()
            .into_iter()
            .find(|diagnostic| diagnostic.code == "RS0207")
            .expect("RS0207 is documented");
        assert!(
            !explanation.explanation.contains("Rust lowering"),
            "{}",
            explanation.explanation
        );
        assert!(explanation.explanation.contains("backend lowering"));
    }

    /// The signature index is generated from the interface sources themselves,
    /// so it can neither invent a function nor miss one.
    #[test]
    fn signatures_come_from_the_interface_sources_and_are_never_truncated() {
        let signatures = interface_signatures();

        // Every public declaration in the prelude reaches the index. The index
        // also carries protocol methods, which are callable contracts declared
        // inside a `protocol` block rather than as a top-level `pub fn`.
        let declared = prelude_interfaces()
            .iter()
            .flat_map(|interface| interface.source.lines())
            .filter_map(|line| {
                let line = line.trim_start();
                line.strip_prefix("pub async fn ")
                    .or_else(|| line.strip_prefix("pub fn "))
            })
            .map(|declaration| {
                declaration
                    .split(['(', '<'])
                    .next()
                    .unwrap_or(declaration)
                    .trim()
                    .to_string()
            })
            .collect::<Vec<_>>();
        assert!(!declared.is_empty());
        for name in &declared {
            assert!(
                signatures.iter().any(|signature| &signature.name == name),
                "`{name}` is declared in an interface but missing from the index"
            );
        }
        assert!(
            signatures.len() >= declared.len(),
            "the index must carry every declared function, with nothing dropped"
        );
        assert!(
            signatures
                .iter()
                .any(|signature| signature.name == "Clone.clone"),
            "protocol methods are callable contracts and belong in the index"
        );

        // A few load-bearing names the failure report singles out.
        for expected in [
            "Output.write(message: String) -> Unit",
            "String.parse_int(value: String) -> Option<Int>",
        ] {
            assert!(
                signatures
                    .iter()
                    .any(|signature| signature.signature == expected),
                "missing `{expected}`"
            );
        }

        let json_fields = signatures
            .iter()
            .filter(|signature| signature.namespace.as_deref() == Some("Json"))
            .map(|signature| signature.name.as_str())
            .collect::<Vec<_>>();
        for field in ["Json.field", "Json.field_int", "Json.field_string"] {
            assert!(json_fields.contains(&field), "missing `{field}`");
        }

        // Grouped by namespace, then by file, so the rendered list stays
        // navigable as it grows past a screenful.
        let document = signatures_document();
        assert!(document.contains("\n## Json\n"));
        assert!(document.contains("`stdlib/json/json.rssi`"));
        assert!(document.contains("- `Output.write(message: String) -> Unit`"));
        for signature in &signatures {
            assert!(
                document.contains(&format!("- `{}`\n", signature.signature)),
                "`{}` is missing from the rendered index",
                signature.signature
            );
        }
    }

    /// The card must *carry* the signatures, not link them.
    ///
    /// Measured on 2026-09-19: `RS0206` appears in 28 of 50 sonnet and 31 of 50
    /// haiku candidates written with the card, the largest class in both
    /// models, while the card showed zero signatures and linked an index
    /// instead. A link is not a prompt. Every signature the standalone index
    /// carries must therefore appear in the card itself, grouped the same way,
    /// with nothing dropped.
    #[test]
    fn the_card_carries_every_signature_inline_and_does_not_merely_link_them() {
        let signatures = interface_signatures();
        let card = language_card_document();

        assert!(card.contains("\n## Core interface signatures\n"));
        // Namespaces nest one level under the card's own section heading.
        assert!(card.contains("\n### Json\n"));
        assert!(card.contains("\n### String\n"));
        assert!(card.contains("`stdlib/json/json.rssi`"));
        for signature in &signatures {
            assert!(
                card.contains(&format!("- `{}`\n", signature.signature)),
                "`{}` is missing from the card itself",
                signature.signature
            );
        }

        // The standalone index keeps its own top-level headings and stays a
        // complete copy, so the two renderings cannot drift apart.
        let index = signatures_document();
        assert!(index.contains("\n## Json\n"));
        assert_eq!(
            index.lines().filter(|line| line.starts_with("- `")).count(),
            signatures.len()
        );
        assert_eq!(
            card.lines().filter(|line| line.starts_with("- `")).count(),
            signatures.len(),
            "the card's inline list must be exactly as long as the index"
        );
        assert_eq!(
            card.matches("\n### ").count(),
            namespace_count(&signatures),
            "one namespace heading per namespace, nested under the card's section"
        );

        // A reference a model reads once, not a file it is told about: the card
        // grows past a screenful on purpose, and the assertion records the
        // size so a silent truncation cannot pass.
        assert!(
            card.lines().count() > 700,
            "the inline signature list should dominate the card: {} lines",
            card.lines().count()
        );
    }

    /// Each canonical surface form must name a real measured failure and render
    /// a right/wrong pair, and the "right" spelling has to be one the parser
    /// actually accepts.
    #[test]
    fn canonical_surface_forms_show_a_right_and_wrong_pair() {
        let forms = canonical_surface_forms();
        assert_eq!(forms.len(), 14);
        let section = canonical_surface_forms_section();
        for form in &forms {
            assert_ne!(form.right, form.wrong);
            // A cell is escaped for the table, so it is the escaped spelling
            // that has to be present — a closure literal is written with the
            // same character Markdown uses to end a column.
            assert!(
                section.contains(&table_cell(form.right)),
                "missing `{}`",
                form.right
            );
            assert!(
                section.contains(&table_cell(form.wrong)),
                "missing `{}`",
                form.wrong
            );
        }
        assert!(
            section.contains("| closure literal | `local double = \\|x\\| { return x * 2 }` |"),
            "the closure row must escape the column separator: {section}"
        );
        // One row per measured class, and the four that are a shape rather
        // than a line are also shown whole.
        for form in [
            "closure literal",
            "closure with an explicit capture list",
            "`with` binds its resource with `as`",
            "`impl` maps an existing function into a protocol slot",
            "dynamic dispatch is built by `Dyn.from`",
            "bind a pattern or leave the block",
        ] {
            assert!(
                forms.iter().any(|candidate| candidate.form == form),
                "`{form}` must be a named surface form"
            );
        }

        let card: serde_json::Value = serde_json::from_str(&language_card_json()).unwrap();
        assert_eq!(
            card["canonical_surface_forms"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            forms.len()
        );
        assert_eq!(
            card["signature_count"].as_u64().unwrap_or_default() as usize,
            interface_signatures().len()
        );
        assert_eq!(
            card["signatures"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            interface_signatures().len(),
            "language-card.json must stay in sync with the rendered index"
        );
    }

    #[test]
    fn canonical_example_is_semantically_valid() {
        let diagnostics =
            rsscript_semantics::analyze_source("language-card.rss", &canonical_example());
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| !diagnostic.severity.is_error()),
            "canonical language-card example must be valid: {diagnostics:#?}"
        );
    }

    /// The worked example for `protocol`/`impl`, `Dyn.from`, `let … else` and
    /// both closure spellings must be a program the compiler accepts *and* the
    /// spelling the formatter prints. A card that shows a form the checker
    /// rejects is worse than a card that stays silent about it.
    #[test]
    fn canonical_forms_example_is_valid_and_already_formatted() {
        let example = canonical_forms_example();
        let diagnostics = rsscript_semantics::analyze_source("language-card.rss", &example);
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| !diagnostic.severity.is_error()),
            "the canonical forms example must check clean: {diagnostics:#?}"
        );
        assert_eq!(
            rsscript_syntax::format_source("language-card.rss", &example),
            example,
            "`rss fmt` must be a fixpoint on the canonical forms example"
        );

        // Each form the example exists to show.
        for form in [
            "protocol Formatter {",
            "impl Formatter for Point {",
            "    format = Point.format",
            "Dyn.from<Formatter, Point>(value: take point)",
            "let Some(base) = value else {",
            "local add = |x| {",
            "local scale = fn(x) captures(read base) {",
        ] {
            assert!(example.contains(form), "the example must show `{form}`");
        }

        assert!(canonical_surface_forms_section().contains(&example));
        let card: serde_json::Value = serde_json::from_str(&language_card_json()).unwrap();
        assert_eq!(card["canonical_forms_example"], example);
    }
}
