//! Labelled arguments reach the parameter they name, whatever order they are
//! written in, and are evaluated in the order they are written.
//!
//! This is the guard for ADR 0244. Lowering used to evaluate a call's
//! arguments in source order and then *pass* them in that order too, so
//! `sub(right: 3, left: 10)` ran as `sub(left: 3, right: 10)`; every such call
//! checked clean, built, and computed a wrong answer. The two orders are
//! separate facts (`CallBinding`'s `evaluation_index` and `parameter_index`),
//! and each call path has to honour both.
//!
//! For one representative callee of every call kind the lowering distinguishes,
//! the test writes the call with its arguments in *every* permutation of their
//! order, each argument expression printing its own label when it runs, and
//! runs the program. For every permutation it asserts:
//!
//! * the result equals the result of the declaration-order call, and
//! * the printed side-effect order equals the written order.
//!
//! An argument that has no expression to hang a side effect on — a `mut`
//! place, a closure literal — is still permuted, so its *placement* is
//! covered; it just contributes no label to the expected evaluation order.

use std::collections::BTreeMap;

use rsscript_sdk::{
    artifact::ArtifactVerifier,
    compile::Compiler,
    provider_api::{
        BlockingBehavior, CancellationBehavior, DataEffect, ExternalSymbol, FunctionSignature,
        ParameterSignature, ProviderCallMode, ProviderDescriptor, ProviderError,
        ProviderErrorMapping, ProviderFunction, ProviderFunctionDescriptor, ProviderRegistry,
        RUNTIME_ABI_VERSION, ResourceCleanupContract, WireInterpreterFn, WireMutationInterpreterFn,
        WireMutationResult, WireValue,
    },
    report::TerminationReason,
    runtime::{ExecutionRequest, Runtime},
};

/// Helpers every generated program shares. `tag` and `stag` print their label
/// when evaluated, which is how the test observes evaluation order.
const PRELUDE: &str = r#"
fn tag(name: String, value: Int) -> Int {
    Output.write(message: name)
    return value
}

fn stag(name: String, value: String) -> fresh String {
    Output.write(message: name)
    return String.copy(value: value)
}

fn tlist(name: String, first: Int, second: Int, third: Int) -> fresh List<Int> {
    Output.write(message: name)
    let values: List<Int> = [first, second, third]
    return values
}
"#;

/// How one argument's expression is written.
#[derive(Clone, Copy)]
enum Arg {
    /// An `Int` expression that prints its label: `tag(name: "<label>", value: N)`.
    Int(i64),
    /// A `String` expression that prints its label.
    Str(&'static str),
    /// A `List<Int>` expression that prints its label.
    List(i64, i64, i64),
    /// An expression that prints the argument's label itself when evaluated.
    Tagged(&'static str),
    /// An expression with no observable evaluation (a `mut` place, a closure
    /// literal, a local). `{i}` is replaced by the call's index.
    Plain(&'static str),
}

struct Case {
    name: &'static str,
    /// Declarations the case needs besides the prelude.
    declarations: &'static str,
    /// Statements run before each call; `{i}` is replaced by the call's index.
    setup: &'static str,
    /// The call, with `{args}` standing for the argument list and `{i}` for
    /// the call's index.
    call: &'static str,
    /// The labelled arguments in declaration order.
    args: &'static [(&'static str, Arg)],
    /// An expression rendering the call's observable result as a `String`;
    /// `{r}` is the result binding and `{i}` the call's index.
    render: &'static str,
    /// Wrap every call in a `task_group` (async calls must be reached there).
    in_task_group: bool,
    /// An `.rssi` interface the program is compiled with, if any.
    interface: Option<&'static str>,
}

fn argument_source(label: &str, arg: Arg, index: usize) -> String {
    let value = match arg {
        Arg::Int(value) => format!("tag(name: \"{label}\", value: {value})"),
        Arg::Str(value) => format!("stag(name: \"{label}\", value: \"{value}\")"),
        Arg::List(first, second, third) => {
            format!("tlist(name: \"{label}\", first: {first}, second: {second}, third: {third})")
        }
        Arg::Tagged(source) | Arg::Plain(source) => source.replace("{i}", &index.to_string()),
    };
    format!("{label}: {value}")
}

fn permutations(count: usize) -> Vec<Vec<usize>> {
    fn extend(prefix: &mut Vec<usize>, remaining: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
        if remaining.is_empty() {
            out.push(prefix.clone());
            return;
        }
        for position in 0..remaining.len() {
            let next = remaining.remove(position);
            prefix.push(next);
            extend(prefix, remaining, out);
            prefix.pop();
            remaining.insert(position, next);
        }
    }
    let mut out = Vec::new();
    extend(&mut Vec::new(), &mut (0..count).collect(), &mut out);
    out
}

/// The program for one case: every permutation of its argument order, each
/// call's output framed by a `#` line before and a `=` line after the call.
fn program(case: &Case, orders: &[Vec<usize>]) -> String {
    let mut body = String::new();
    for (index, order) in orders.iter().enumerate() {
        let args = order
            .iter()
            .map(|&slot| {
                let (label, arg) = case.args[slot];
                argument_source(label, arg, index)
            })
            .collect::<Vec<_>>()
            .join(", ");
        let setup = case
            .setup
            .replace("{args}", &args)
            .replace("{i}", &index.to_string());
        let call = case
            .call
            .replace("{args}", &args)
            .replace("{i}", &index.to_string());
        let render = case
            .render
            .replace("{r}", &format!("r{index}"))
            .replace("{i}", &index.to_string());
        body.push_str(&format!(
            "    Output.write(message: \"#\")\n    {setup}\n    let r{index} = {call}\n    Output.write(message: \"=\")\n    Output.write(message: {render})\n"
        ));
    }
    let body = if case.in_task_group {
        format!("    task_group {{\n{body}    }}\n")
    } else {
        body
    };
    format!(
        "{PRELUDE}\n{}\nfn main() -> Unit {{\n{body}    return Unit\n}}\n",
        case.declarations
    )
}

/// One call's observed output: the labels its arguments printed, in the
/// order they printed them, and its rendered result.
#[derive(Debug, PartialEq)]
struct Observed {
    evaluated: Vec<String>,
    result: String,
}

fn parse(stdout: &str) -> Vec<Observed> {
    let mut calls = Vec::new();
    let mut lines = stdout.lines().peekable();
    while let Some(line) = lines.next() {
        assert_eq!(line, "#", "every call's output starts with `#`:\n{stdout}");
        let mut evaluated = Vec::new();
        for line in lines.by_ref() {
            if line == "=" {
                break;
            }
            evaluated.push(line.to_owned());
        }
        let mut result = Vec::new();
        while let Some(line) = lines.next_if(|line| *line != "#") {
            result.push(line);
        }
        calls.push(Observed {
            evaluated,
            result: result.join("\n"),
        });
    }
    calls
}

fn registry() -> ProviderRegistry {
    let int = |name: &str, effect: DataEffect| ParameterSignature {
        name: name.into(),
        effect,
        ty: "Int".into(),
        retained: false,
    };
    let descriptor =
        |symbol: &ExternalSymbol, signature: &FunctionSignature, entry: &str| ProviderDescriptor {
            provider_id: format!("order.test.{entry}"),
            provider_version: "1".into(),
            supported_abi: vec![RUNTIME_ABI_VERSION],
            record_layouts: Vec::new(),
            variant_layouts: Vec::new(),
            functions: vec![ProviderFunctionDescriptor {
                symbol: symbol.clone(),
                signature: signature.clone(),
                entry: entry.into(),
                call_mode: ProviderCallMode::Sync,
                blocking: BlockingBehavior::NonBlocking,
                cancellation: CancellationBehavior::NotApplicable,
                thread_safe: true,
                reentrant: true,
                resource_cleanup: ResourceCleanupContract::None,
                error_mapping: ProviderErrorMapping::StructuredV1,
            }],
        };
    let mut providers = ProviderRegistry::default();

    // `combine(a, b, c)`: a pure read-only external call.
    let combine = ExternalSymbol::new("host.order.combine").expect("symbol");
    let signature = FunctionSignature {
        parameters: vec![
            int("a", DataEffect::Read),
            int("b", DataEffect::Read),
            int("c", DataEffect::Read),
        ],
        result: "Int".into(),
        asynchronous: false,
    };
    providers
        .register(
            &descriptor(&combine, &signature, "combine"),
            BTreeMap::from([(
                combine,
                ProviderFunction {
                    signature,
                    callable: WireInterpreterFn::new(|args| match args.as_slice() {
                        [
                            WireValue::Int { value: a },
                            WireValue::Int { value: b },
                            WireValue::Int { value: c },
                        ] => Ok(WireValue::Int {
                            value: a * 100 + b * 10 + c,
                        }),
                        _ => Err(ProviderError::invalid_argument(
                            "combine expects three Ints",
                        )),
                    }),
                },
            )]),
        )
        .expect("combine Provider matches its descriptor");

    // `bump(counter: mut Int, by, scale)`: the `mut` write-back goes to the
    // place passed as `counter`, wherever the caller wrote it.
    let bump = ExternalSymbol::new("host.order.bump").expect("symbol");
    let signature = FunctionSignature {
        parameters: vec![
            int("counter", DataEffect::Mut),
            int("by", DataEffect::Read),
            int("scale", DataEffect::Read),
        ],
        result: "Int".into(),
        asynchronous: false,
    };
    providers
        .register(
            &descriptor(&bump, &signature, "bump"),
            BTreeMap::from([(
                bump,
                ProviderFunction {
                    signature,
                    callable: WireMutationInterpreterFn::new(|args| match args.as_slice() {
                        [
                            WireValue::Int { value: counter },
                            WireValue::Int { value: by },
                            WireValue::Int { value: scale },
                        ] => Ok(WireMutationResult {
                            result: WireValue::Int {
                                value: by * 10 + scale,
                            },
                            mutated: vec![WireValue::Int {
                                value: counter * 1000 + by * 10 + scale,
                            }],
                        }),
                        _ => Err(ProviderError::invalid_argument("bump expects three Ints")),
                    }),
                },
            )]),
        )
        .expect("bump Provider matches its descriptor");
    providers
}

const HOST_INTERFACE: &str = "module host.order\n\
pub fn combine(a: read Int, b: read Int, c: read Int) -> Int\n\
pub fn bump(counter: mut Int, by: read Int, scale: read Int) -> Int\n";

const INT_RESULT: &str = "Int.to_string(value: {r})";

const CASES: &[Case] = &[
    Case {
        name: "private user function, four parameters",
        declarations: "fn digits(a: Int, b: Int, c: Int, d: Int) -> Int { return a * 1000 + b * 100 + c * 10 + d }",
        setup: "",
        call: "digits({args})",
        args: &[
            ("a", Arg::Int(1)),
            ("b", Arg::Int(2)),
            ("c", Arg::Int(3)),
            ("d", Arg::Int(4)),
        ],
        render: INT_RESULT,
        in_task_group: false,
        interface: None,
    },
    Case {
        name: "pub user function with mixed parameter types",
        declarations: "pub fn describe(name: String, count: Int, suffix: String) -> fresh String { return $\"{name}:{Int.to_string(value: count)}:{suffix}\" }",
        setup: "",
        call: "describe({args})",
        args: &[
            ("name", Arg::Str("n")),
            ("count", Arg::Int(7)),
            ("suffix", Arg::Str("s")),
        ],
        render: "{r}",
        in_task_group: false,
        interface: None,
    },
    Case {
        name: "defaulted parameter declared before supplied ones",
        declarations: "fn defaulted(left: Int, middle: Int = 5, right: Int) -> Int { return left * 100 + middle * 10 + right }",
        setup: "",
        call: "defaulted({args})",
        args: &[("left", Arg::Int(1)), ("right", Arg::Int(3))],
        render: INT_RESULT,
        in_task_group: false,
        interface: None,
    },
    Case {
        name: "defaulted parameter supplied",
        declarations: "fn defaulted(left: Int, middle: Int = 5, right: Int) -> Int { return left * 100 + middle * 10 + right }",
        setup: "",
        call: "defaulted({args})",
        args: &[
            ("left", Arg::Int(1)),
            ("middle", Arg::Int(2)),
            ("right", Arg::Int(3)),
        ],
        render: INT_RESULT,
        in_task_group: false,
        interface: None,
    },
    Case {
        name: "user function with a mut parameter",
        declarations: "fn place(values: mut List<Int>, index: Int, value: Int) -> Int { List.set(list: mut values, index: index, value: value)\n return index * 10 + value }",
        setup: "let mut xs{i}: List<Int> = [0, 0, 0]",
        call: "place({args})",
        args: &[
            ("values", Arg::Plain("mut xs{i}")),
            ("index", Arg::Int(1)),
            ("value", Arg::Int(9)),
        ],
        render: "$\"{Int.to_string(value: {r})} {Int.to_string(value: List.get(list: xs{i}, index: 0))}{Int.to_string(value: List.get(list: xs{i}, index: 1))}\"",
        in_task_group: false,
        interface: None,
    },
    Case {
        name: "generic function instance",
        declarations: "fn choose<T>(first: read T, second: read T, use_first: Bool) -> T { if use_first { return first }\n return second }",
        setup: "",
        call: "choose<Int>({args})",
        args: &[
            ("first", Arg::Int(1)),
            ("second", Arg::Int(2)),
            ("use_first", Arg::Plain("false")),
        ],
        render: INT_RESULT,
        in_task_group: false,
        interface: None,
    },
    Case {
        name: "qualified method call with the receiver permuted",
        declarations: "struct Acc { base: Int }\nfn make_acc(base: Int) -> fresh Acc { Output.write(message: \"self\")\n return Acc(base: base) }\nfn Acc.combine(self: read Acc, a: Int, b: Int, c: Int) -> Int { return self.base + a * 100 + b * 10 + c }",
        setup: "",
        call: "Acc.combine({args})",
        args: &[
            ("self", Arg::Tagged("make_acc(base: 5000)")),
            ("a", Arg::Int(1)),
            ("b", Arg::Int(2)),
            ("c", Arg::Int(3)),
        ],
        render: INT_RESULT,
        in_task_group: false,
        interface: None,
    },
    Case {
        name: "receiver-call sugar on a user method",
        declarations: "struct Acc { base: Int }\nfn Acc.combine(self: read Acc, a: Int, b: Int, c: Int) -> Int { return self.base + a * 100 + b * 10 + c }",
        setup: "let acc{i} = Acc(base: 5000)",
        call: "acc{i}.combine({args})",
        args: &[("a", Arg::Int(1)), ("b", Arg::Int(2)), ("c", Arg::Int(3))],
        render: INT_RESULT,
        in_task_group: false,
        interface: None,
    },
    Case {
        name: "record constructor",
        declarations: "struct Triple {\n    a: Int\n    b: Int\n    c: Int\n}",
        setup: "",
        call: "Triple({args})",
        args: &[("a", Arg::Int(1)), ("b", Arg::Int(2)), ("c", Arg::Int(3))],
        render: "Int.to_string(value: {r}.a * 100 + {r}.b * 10 + {r}.c)",
        in_task_group: false,
        interface: None,
    },
    Case {
        name: "sum-variant constructor",
        declarations: "sum Shape {\n    Box(a: Int, b: Int, c: Int)\n    Empty\n}\nfn weigh(shape: read Shape) -> Int { match read shape { Box(a, b, c) => { return a * 100 + b * 10 + c } Empty => { return 0 } } }",
        setup: "",
        call: "Box({args})",
        args: &[("a", Arg::Int(1)), ("b", Arg::Int(2)), ("c", Arg::Int(3))],
        render: "Int.to_string(value: weigh(shape: {r}))",
        in_task_group: false,
        interface: None,
    },
    Case {
        name: "special builtin String.concat",
        declarations: "",
        setup: "",
        call: "String.concat({args})",
        args: &[("left", Arg::Str("L")), ("right", Arg::Str("R"))],
        render: "{r}",
        in_task_group: false,
        interface: None,
    },
    Case {
        name: "special builtin List.get",
        declarations: "",
        setup: "",
        call: "List.get({args})",
        args: &[("list", Arg::List(10, 20, 30)), ("index", Arg::Int(2))],
        render: INT_RESULT,
        in_task_group: false,
        interface: None,
    },
    Case {
        name: "special builtin List.set with a mut place",
        declarations: "",
        setup: "let mut xs{i}: List<Int> = [0, 0, 0]",
        call: "List.set({args})",
        args: &[
            ("list", Arg::Plain("mut xs{i}")),
            ("index", Arg::Int(1)),
            ("value", Arg::Int(9)),
        ],
        render: "$\"{Int.to_string(value: List.get(list: xs{i}, index: 0))}{Int.to_string(value: List.get(list: xs{i}, index: 1))}\"",
        in_task_group: false,
        interface: None,
    },
    Case {
        name: "special builtin Map.insert with a mut place",
        declarations: "",
        setup: "let mut m{i} = Map.new<Int, Int>()",
        call: "Map.insert({args})",
        args: &[
            ("map", Arg::Plain("mut m{i}")),
            ("key", Arg::Int(1)),
            ("value", Arg::Int(9)),
        ],
        render: "$\"{Int.to_string(value: Option.unwrap_or(value: Map.get(map: m{i}, key: 1), default: 0 - 1))}{Int.to_string(value: Option.unwrap_or(value: Map.get(map: m{i}, key: 9), default: 0 - 1))}\"",
        in_task_group: false,
        interface: None,
    },
    Case {
        name: "closure-taking builtin List.map",
        declarations: "",
        setup: "",
        call: "List.map({args})",
        args: &[
            ("list", Arg::List(1, 2, 3)),
            ("mapper", Arg::Plain("|x| { return x * 10 }")),
        ],
        render: "Int.to_string(value: List.sum(list: {r}) * 1000 + List.get(list: {r}, index: 0))",
        in_task_group: false,
        interface: None,
    },
    Case {
        name: "direct builtin String.replace",
        declarations: "",
        setup: "",
        call: "String.replace({args})",
        args: &[
            ("value", Arg::Str("abcabc")),
            ("from", Arg::Str("b")),
            ("to", Arg::Str("X")),
        ],
        render: "{r}",
        in_task_group: false,
        interface: None,
    },
    Case {
        name: "direct builtin String.pad_left with mixed types",
        declarations: "",
        setup: "",
        call: "String.pad_left({args})",
        args: &[
            ("value", Arg::Str("7")),
            ("width", Arg::Int(4)),
            ("fill", Arg::Str("0")),
        ],
        render: "{r}",
        in_task_group: false,
        interface: None,
    },
    Case {
        name: "direct builtin List.slice",
        declarations: "",
        setup: "",
        call: "List.slice({args})",
        args: &[
            ("list", Arg::List(10, 20, 30)),
            ("start", Arg::Int(1)),
            ("len", Arg::Int(2)),
        ],
        render: "Int.to_string(value: List.sum(list: {r}) * 100 + List.len(list: {r}))",
        in_task_group: false,
        interface: None,
    },
    Case {
        name: "direct builtin Math.clamp",
        declarations: "",
        setup: "",
        call: "Math.clamp({args})",
        args: &[
            ("value", Arg::Int(50)),
            ("min", Arg::Int(1)),
            ("max", Arg::Int(9)),
        ],
        render: INT_RESULT,
        in_task_group: false,
        interface: None,
    },
    Case {
        name: "receiver-call sugar on a builtin",
        declarations: "",
        setup: "let s{i} = \"abcabc\"",
        call: "s{i}.replace({args})",
        args: &[("from", Arg::Str("b")), ("to", Arg::Str("X"))],
        render: "{r}",
        in_task_group: false,
        interface: None,
    },
    Case {
        name: "receiver-call sugar on a mutating builtin",
        declarations: "",
        setup: "let mut xs{i}: List<Int> = [0, 0, 0]",
        call: "mut xs{i}.set({args})",
        args: &[("index", Arg::Int(1)), ("value", Arg::Int(9))],
        render: "$\"{Int.to_string(value: List.get(list: xs{i}, index: 0))}{Int.to_string(value: List.get(list: xs{i}, index: 1))}\"",
        in_task_group: false,
        interface: None,
    },
    Case {
        name: "dynamic protocol dispatch through Dyn<P>",
        declarations: "protocol Weigh { fn weigh(self: read Self, a: Int, b: Int, c: Int) -> Int }\nstruct Scale { base: Int }\nfn Scale.weigh(self: read Scale, a: Int, b: Int, c: Int) -> Int { return self.base + a * 100 + b * 10 + c }\nimpl Weigh for Scale { weigh = Scale.weigh }",
        setup: "local scale{i} = Scale(base: 5000)\n    let boxed{i} = Dyn.from<Weigh, Scale>(value: take scale{i})",
        call: "Weigh.weigh({args})",
        args: &[
            ("self", Arg::Plain("boxed{i}")),
            ("a", Arg::Int(1)),
            ("b", Arg::Int(2)),
            ("c", Arg::Int(3)),
        ],
        render: INT_RESULT,
        in_task_group: false,
        interface: None,
    },
    Case {
        name: "static protocol dispatch under a bound",
        declarations: "protocol Weigh { fn weigh(self: read Self, a: Int, b: Int, c: Int) -> Int }\nstruct Scale { base: Int }\nfn Scale.weigh(self: read Scale, a: Int, b: Int, c: Int) -> Int { return self.base + a * 100 + b * 10 + c }\nimpl Weigh for Scale { weigh = Scale.weigh }\nfn weigh_any<T: Weigh>(value: read T, a: Int, b: Int, c: Int) -> Int { return Weigh.weigh(self: value, c: c, a: a, b: b) }",
        setup: "let scale{i} = Scale(base: 5000)",
        call: "weigh_any<Scale>({args})",
        args: &[
            ("value", Arg::Plain("scale{i}")),
            ("a", Arg::Int(1)),
            ("b", Arg::Int(2)),
            ("c", Arg::Int(3)),
        ],
        render: INT_RESULT,
        in_task_group: false,
        interface: None,
    },
    Case {
        name: "async let",
        declarations: "async fn digits(a: Int, b: Int, c: Int) -> Int { return a * 100 + b * 10 + c }",
        setup: "async let h{i} = digits({args})",
        call: "await h{i}",
        args: &[("a", Arg::Int(1)), ("b", Arg::Int(2)), ("c", Arg::Int(3))],
        render: INT_RESULT,
        in_task_group: true,
        interface: None,
    },
    Case {
        name: "external Provider call",
        declarations: "",
        setup: "",
        call: "combine({args})",
        args: &[("a", Arg::Int(1)), ("b", Arg::Int(2)), ("c", Arg::Int(3))],
        render: INT_RESULT,
        in_task_group: false,
        interface: Some(HOST_INTERFACE),
    },
    Case {
        name: "external Provider call with a mut write-back",
        declarations: "",
        setup: "let mut counter{i} = 4",
        call: "bump({args})",
        args: &[
            ("counter", Arg::Plain("mut counter{i}")),
            ("by", Arg::Int(2)),
            ("scale", Arg::Int(3)),
        ],
        render: "$\"{Int.to_string(value: {r})} {Int.to_string(value: counter{i})}\"",
        in_task_group: false,
        interface: Some(HOST_INTERFACE),
    },
];

/// Build and run one case; return every permutation that went wrong, so a
/// failure names each call kind that binds or evaluates out of order.
fn run_case(case: &Case) -> Vec<String> {
    let orders = permutations(case.args.len());
    let source = program(case, &orders);
    let module = if case.interface.is_some() {
        format!("module app\nuse host.order.*\n{source}")
    } else {
        source
    };
    let interfaces = case
        .interface
        .map(|interface| vec![("host_order.rssi", interface)])
        .unwrap_or_default();
    // The source always checks; a build, verification, or link failure is a
    // failure of this case (verification caught some wrong bindings when the
    // misplaced argument's type differed from the parameter's).
    let built =
        match Compiler.compile_with_interfaces(&[("main.rss", module.as_str())], &interfaces) {
            Ok(built) => built,
            Err(error) => {
                return vec![format!(
                    "{}: compile failed: {error:?}\n{module}",
                    case.name
                )];
            }
        };
    let admitted = match ArtifactVerifier.verify(built) {
        Ok(verified) => verified.admit_trusted_input(),
        Err(error) => return vec![format!("{}: verify failed: {error}", case.name)],
    };
    let linked = match Runtime::new(registry()).link(&admitted) {
        Ok(linked) => linked,
        Err(error) => return vec![format!("{}: link failed: {error}", case.name)],
    };
    let report = linked.execute(ExecutionRequest::default());
    if report.termination_reason() != TerminationReason::Completed {
        return vec![format!(
            "{}: did not complete: {:?}\n{}",
            case.name,
            report.outcome(),
            report.stdout
        )];
    }
    let observed = parse(&report.stdout);
    assert_eq!(
        observed.len(),
        orders.len(),
        "{}: {}",
        case.name,
        report.stdout
    );
    let expected_result = &observed[0].result;
    let mut failures = Vec::new();
    for (order, call) in orders.iter().zip(&observed) {
        let written = order
            .iter()
            .map(|&slot| case.args[slot].0)
            .collect::<Vec<_>>();
        if &call.result != expected_result {
            failures.push(format!(
                "{}: written ({}) computed {:?}, the declaration-order call computed {:?}",
                case.name,
                written.join(", "),
                call.result,
                expected_result
            ));
        }
        let expected_evaluation = order
            .iter()
            .map(|&slot| case.args[slot])
            .filter_map(|(label, arg)| match arg {
                Arg::Plain(_) => None,
                _ => Some(label),
            })
            .collect::<Vec<_>>();
        if call.evaluated != expected_evaluation {
            failures.push(format!(
                "{}: written ({}) evaluated ({})",
                case.name,
                written.join(", "),
                call.evaluated.join(", ")
            ));
        }
    }
    failures
}

#[test]
fn labelled_arguments_bind_by_name_and_evaluate_as_written_for_every_call_kind() {
    let failures = CASES.iter().flat_map(run_case).collect::<Vec<_>>();
    assert!(
        failures.is_empty(),
        "{} permutation(s) went wrong:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The `select` arm spawns its operation through the same lowering as
/// `async let`; permute the arm's arguments too.
#[test]
fn select_arm_arguments_bind_by_name_and_evaluate_as_written() {
    let orders = permutations(3);
    let labels = ["a", "b", "c"];
    let mut body = String::new();
    for (index, order) in orders.iter().enumerate() {
        let args = order
            .iter()
            .map(|&slot| argument_source(labels[slot], Arg::Int(slot as i64 + 1), index))
            .collect::<Vec<_>>()
            .join(", ");
        body.push_str(&format!(
            "    Output.write(message: \"#\")\n    let mut r{index} = 0\n    task_group {{\n        select {{\n            value = await digits({args}) => {{ r{index} = value }}\n        }}\n    }}\n    Output.write(message: \"=\")\n    Output.write(message: Int.to_string(value: r{index}))\n"
        ));
    }
    let source = format!(
        "{PRELUDE}\nasync fn digits(a: Int, b: Int, c: Int) -> Int {{ return a * 100 + b * 10 + c }}\nfn main() -> Unit {{\n{body}    return Unit\n}}\n"
    );
    let built = Compiler
        .compile("main.rss", &source)
        .unwrap_or_else(|error| panic!("compile failed: {error}\n{source}"));
    let admitted = ArtifactVerifier
        .verify(built)
        .expect("verify")
        .admit_trusted_input();
    let report = Runtime::default()
        .link(&admitted)
        .expect("link")
        .execute(ExecutionRequest::default());
    assert_eq!(report.termination_reason(), TerminationReason::Completed);
    let observed = parse(&report.stdout);
    assert_eq!(observed.len(), orders.len());
    for (order, call) in orders.iter().zip(&observed) {
        let written = order.iter().map(|&slot| labels[slot]).collect::<Vec<_>>();
        assert_eq!(
            call.result,
            "123",
            "select arm written ({})",
            written.join(", ")
        );
        assert_eq!(call.evaluated, written);
    }
}

/// A same-name argument (`sub(right, left)` for `sub(left:, right:)`) binds
/// by name wherever it is written (§4.4), so it is placed like a labelled one.
#[test]
fn same_name_arguments_written_out_of_order_bind_by_name() {
    const SOURCE: &str = r#"
fn sub(left: Int, right: Int) -> Int {
    return left - right
}

pub fn pad(value: String, width: Int, fill: String) -> fresh String {
    return String.pad_left(value: value, width: width, fill: fill)
}

fn main() -> Unit {
    let left = 10
    let right = 3
    let fill = "0"
    let value = "7"
    Output.write(message: Int.to_string(value: sub(right, left)))
    Output.write(message: pad(fill, value, width: 4))
    Output.write(message: String.pad_left(fill, width: 4, value))
    return Unit
}
"#;
    let built = Compiler.compile("main.rss", SOURCE).expect("compile");
    let admitted = ArtifactVerifier
        .verify(built)
        .expect("verify")
        .admit_trusted_input();
    let report = Runtime::default()
        .link(&admitted)
        .expect("link")
        .execute(ExecutionRequest::default());
    assert_eq!(report.termination_reason(), TerminationReason::Completed);
    assert_eq!(report.stdout, "7\n0007\n0007\n");
}
