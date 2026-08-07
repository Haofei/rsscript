// ---------------------------------------------------------------------------
// Phase 2 — parser recognition parity.
//
// The rss parser (`selfhost/parser.rss`) recognizes rss source and prints a
// verdict: `OK` if it accepts, or `ERR <line> <col>` at the first syntax error.
// Oracle: the real Rust parser `crate::syntax::parse_source_raw`, which never
// panics and collects parse errors as span vectors on the returned `Program`.
// Recognition tier (default): compare accept-vs-reject only. Position tier
// (`RSS_SELFHOST_PARSE_TIER=1`): also compare the first-error line:col.
// ---------------------------------------------------------------------------

/// Oracle verdict: `None` if the Rust parser accepts, else the first parse
/// error's (line, column).
fn parse_oracle_error(file: &str, source: &str) -> Option<(usize, usize)> {
    let program = crate::syntax::parse_source_raw(file, source);
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for s in &program.unknown_top_level_spans {
        spans.push((s.line, s.column));
    }
    for s in &program.malformed_declaration_spans {
        spans.push((s.line, s.column));
    }
    spans.sort_unstable();
    spans.into_iter().next()
}

fn parse_position_tier() -> bool {
    env_flag_tier("RSS_SELFHOST_PARSE_TIER")
}

fn compile_parser() -> Result<RegVmExecutable, String> {
    compile_selfhost_tool("parser.rss", "parser")
}

/// Run the precompiled rss parser; parse its verdict line.
fn run_parser(exe: &RegVmExecutable, source: &str) -> Result<Option<(usize, usize)>, String> {
    let output = exe
        .eval_main_with_args([source.to_string()])
        .map_err(|e| format!("rss parser failed to run: {e:?}"))?;
    parse_parser_output(&output.stdout)
}

fn parse_parser_output(stdout: &str) -> Result<Option<(usize, usize)>, String> {
    let lines = stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>();
    if lines.len() != 1 {
        return Err(format!(
            "rss parser must emit exactly one non-empty verdict line, got {}",
            lines.len()
        ));
    }
    let verdict = lines[0];
    if verdict == "OK" {
        Ok(None)
    } else if let Some(rest) = verdict.strip_prefix("ERR ") {
        let mut nums = rest.split_whitespace();
        let line = nums
            .next()
            .ok_or_else(|| format!("missing parser error line in verdict: {verdict:?}"))?
            .parse::<usize>()
            .map_err(|_| format!("invalid parser error line in verdict: {verdict:?}"))?;
        let col = nums
            .next()
            .ok_or_else(|| format!("missing parser error column in verdict: {verdict:?}"))?
            .parse::<usize>()
            .map_err(|_| format!("invalid parser error column in verdict: {verdict:?}"))?;
        if nums.next().is_some() || line == 0 || col == 0 {
            return Err(format!("invalid parser error verdict: {verdict:?}"));
        }
        Ok(Some((line, col)))
    } else {
        Err(format!("unrecognized parser verdict: {verdict:?}"))
    }
}

#[test]
fn parser_output_parser_rejects_malformed_verdicts() {
    assert_eq!(parse_parser_output("OK\n").unwrap(), None);
    assert_eq!(parse_parser_output("ERR 2 3\n").unwrap(), Some((2, 3)));
    assert!(parse_parser_output("").is_err());
    assert!(parse_parser_output("debug\nOK\n").is_err());
    assert!(parse_parser_output("ERR\n").is_err());
    assert!(parse_parser_output("ERR bad\n").is_err());
    assert!(parse_parser_output("ERR 0 3\n").is_err());
    assert!(parse_parser_output("ERR 2 3 4\n").is_err());
}

/// Compare parser verdicts. Recognition tier: accept-vs-reject. Position tier:
/// also the first-error coordinates.
fn compare_parse(
    oracle: Option<(usize, usize)>,
    actual: Option<(usize, usize)>,
    position: bool,
) -> Result<(), String> {
    if oracle.is_some() != actual.is_some() {
        return Err(format!(
            "accept/reject diverges: oracle={:?} rss={:?}",
            oracle, actual
        ));
    }
    if position && oracle != actual {
        return Err(format!(
            "first-error position diverges: oracle={:?} rss={:?}",
            oracle, actual
        ));
    }
    Ok(())
}

/// Phase-2 proof: the rss parser agrees with the Rust parser on a tiny sample.
#[test]
fn parser_parity_tiny_sample() {
    let sample_path = selfhost_dir().join("samples/tiny.rss");
    let source = std::fs::read_to_string(&sample_path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", sample_path.display()));
    let oracle = parse_oracle_error("samples/tiny.rss", &source);
    let exe = compile_parser().expect("rss parser should compile");
    let actual = run_parser(&exe, &source).expect("rss parser should run");
    compare_parse(oracle, actual, parse_position_tier()).unwrap_or_else(|msg| panic!("{msg}"));
}


#[test]
fn selfhost_data_declaration_ast_outline_is_deterministic() {
    let source = r#"pub opaque resource Cache<T: Resource> derives(Clone, Eq) {
    id: Int
    owner: handle Owner
    peer: weak Peer
}
pub sum Resultish<T> derives(Eq) {
    Good(value: T, code: Int)
    Empty
}
"#;
    let exe = compile_selfhost_tool("serialize/outline.rss", "data declaration AST outline")
        .expect("data declaration AST outline should compile");
    let output = exe
        .eval_main_with_args([source.to_string()])
        .expect("data declaration AST outline should run");
    assert_eq!(
        output.stdout,
        concat!(
            "type\tCache\t1:1:3\n",
            "  data-kind\tresource\tpublic=true\topaque=true\n",
            "  generic\tT\tResource\n",
            "  derive\tClone\n",
            "  derive\tEq\n",
            "  field\tid\tInt\thandle=false\tweak=false\n",
            "  field\towner\tOwner\thandle=true\tweak=false\n",
            "  field\tpeer\tPeer\thandle=false\tweak=true\n",
            "sum\tResultish\t6:1:3\n",
            "  data-kind\tsum\tpublic=true\topaque=false\n",
            "  generic\tT\t\n",
            "  derive\tEq\n",
            "  variant\tGood\n",
            "    field\tvalue\tT\thandle=false\tweak=false\n",
            "    field\tcode\tInt\thandle=false\tweak=false\n",
            "  variant\tEmpty\n",
        )
    );
}

#[test]
fn selfhost_protocol_contract_ast_outline_is_deterministic() {
    let source = r#"protocol Writer {
    fn write(self: mut Self, message: read String) -> Unit
    fn default_write(self: read Self) -> Unit = _
}
struct Buffer {}
fn Buffer.write(self: mut Buffer, message: read String) -> Unit {
    return Unit
}
fn Buffer.default_write(self: read Buffer) -> Unit {
    return Unit
}
impl Writer for Buffer {
    write = Buffer.write
    default_write = Buffer.default_write
}
"#;
    let exe = compile_selfhost_tool("serialize/outline.rss", "protocol contract AST outline")
        .expect("protocol contract AST outline should compile");
    let output = exe
        .eval_main_with_args([source.to_string()])
        .expect("protocol contract AST outline should run");
    assert_eq!(
        output.stdout,
        concat!(
            "protocol\tWriter\t1:1:8\n",
            "function\tWriter.write\t2:5:2\n",
            "  header\tpublic=true\tasync=false\tbody=false\treturn=Unit\tprotocol=Writer\n",
            "  generic\tSelf\tManaged\n",
            "  param\tself\tmut\tSelf\n",
            "  param\tmessage\tread\tString\n",
            "function\tWriter.default_write\t3:5:2\n",
            "  header\tpublic=true\tasync=false\tbody=false\treturn=Unit\tprotocol=Writer\tdefault-impl=true\n",
            "  generic\tSelf\tManaged\n",
            "  param\tself\tread\tSelf\n",
            "type\tBuffer\t5:1:6\n",
            "  data-kind\tstruct\tpublic=false\topaque=false\n",
            "function\tBuffer.write\t6:1:2\n",
            "  header\tpublic=false\tasync=false\tbody=true\treturn=Unit\n",
            "  param\tself\tmut\tBuffer\n",
            "  param\tmessage\tread\tString\n",
            "  stmt\treturn\t\t\tliteral\tUnit\n",
            "function\tBuffer.default_write\t9:1:2\n",
            "  header\tpublic=false\tasync=false\tbody=true\treturn=Unit\n",
            "  param\tself\tread\tBuffer\n",
            "  stmt\treturn\t\t\tliteral\tUnit\n",
            "impl\tWriter\t12:1:4\n",
            "  protocol\tWriter\tfor\tBuffer\n",
            "  mapping\twrite\tBuffer.write\t13:5:5\n",
            "  mapping\tdefault_write\tBuffer.default_write\t14:5:13\n",
        )
    );
}

#[test]
fn selfhost_top_level_ast_outline_is_deterministic() {
    let source = r#"module demo.core
use demo.util.*
struct Boxed {
    value: Int
}
sum Resultish {
    Good
}
type Name = String
const LIMIT: Int = 3
fn run() -> Unit {
    return Unit
}
pub struct PublicBox {
    value: Int
}
async fn async_run() -> Unit {
    return Unit
}
fn ordinary() -> Unit {
    return Unit
}
pub fn external<T: Display, U>(value: take List<String>, count: mut Int = 1) -> fresh Result<Int, String>
#lower_name("lowered_named")
pub fn pinned_name() -> Unit {
    return Unit
}
"#;
    let exe = compile_selfhost_tool("serialize/outline.rss", "top-level AST outline")
        .expect("top-level AST outline should compile");
    let output = exe
        .eval_main_with_args([source.to_string()])
        .expect("top-level AST outline should run");
    assert_eq!(
        output.stdout,
        concat!(
            "module\tdemo.core\t1:1:6\n",
            "use\tdemo.util\t2:1:3\n",
            "type\tBoxed\t3:1:6\n",
            "  data-kind\tstruct\tpublic=false\topaque=false\n",
            "  field\tvalue\tInt\thandle=false\tweak=false\n",
            "sum\tResultish\t6:1:3\n",
            "  data-kind\tsum\tpublic=false\topaque=false\n",
            "  variant\tGood\n",
            "type-alias\tName\t9:1:4\n",
            "  target\tString\n",
            "const\tLIMIT\t10:1:5\n",
            "  type\tInt\n",
            "  value\t3\n",
            "function\trun\t11:1:2\n",
            "  header\tpublic=false\tasync=false\tbody=true\treturn=Unit\n",
            "  stmt\treturn\t\t\tliteral\tUnit\n",
            "type\tPublicBox\t14:1:3\n",
            "  data-kind\tstruct\tpublic=true\topaque=false\n",
            "  field\tvalue\tInt\thandle=false\tweak=false\n",
            "function\tasync_run\t17:1:5\n",
            "  header\tpublic=false\tasync=true\tbody=true\treturn=Unit\n",
            "  stmt\treturn\t\t\tliteral\tUnit\n",
            "function\tordinary\t20:1:2\n",
            "  header\tpublic=false\tasync=false\tbody=true\treturn=Unit\n",
            "  stmt\treturn\t\t\tliteral\tUnit\n",
            "function\texternal\t23:1:3\n",
            "  header\tpublic=true\tasync=false\tbody=false\treturn=fresh Result<Int, String>\n",
            "  generic\tT\tDisplay\n",
            "  generic\tU\t\n",
            "  param\tvalue\ttake\tList<String>\n",
            "  param\tcount\tmut\tInt\n",
            "  default\t23:75:1\n",
            "function\tpinned_name\t24:1:1\n",
            "  header\tpublic=true\tasync=false\tbody=true\treturn=Unit\n",
            "  stmt\treturn\t\t\tliteral\tUnit\n",
        )
    );
}

#[test]
fn selfhost_function_body_ast_outline_is_deterministic() {
    let source = r#"fn consume(value: read Int) -> Unit {
    return Unit
}

fn work() -> Unit {
    let item: Int = 1
    consume(
        value: item
    )
    if item == 1 {
        return item
    } else {
        return Unit
    }
    return item
}
"#;
    let exe = compile_selfhost_tool("serialize/outline.rss", "function-body AST outline")
        .expect("function-body AST outline should compile");
    let output = exe
        .eval_main_with_args([source.to_string()])
        .expect("function-body AST outline should run");
    assert_eq!(
        output.stdout,
        concat!(
            "function\tconsume\t1:1:2\n",
            "  header\tpublic=false\tasync=false\tbody=true\treturn=Unit\n",
            "  param\tvalue\tread\tInt\n",
            "  stmt\treturn\t\t\tliteral\tUnit\n",
            "function\twork\t5:1:2\n",
            "  header\tpublic=false\tasync=false\tbody=true\treturn=Unit\n",
            "  stmt\tlet\titem\tInt\tliteral\t1\n",
            "  stmt\texpr\t\t\tcall\tconsume\targs=1\tlabels=value\n",
            "  stmt\tif\t\t\tbinary\t==\tthen=1\telse=1\n",
            "  stmt\treturn\t\t\tname\titem\n",
        )
    );
}

#[test]
fn selfhost_pipe_closure_ast_outline_is_deterministic() {
    let source = r#"fn incrementer() -> Unit {
    let increment = |value| value + 1
    return Unit
}
"#;
    let exe = compile_selfhost_tool("serialize/outline.rss", "pipe closure AST outline")
        .expect("pipe closure AST outline should compile");
    let output = exe
        .eval_main_with_args([source.to_string()])
        .expect("pipe closure AST outline should run");
    assert_eq!(
        output.stdout,
        concat!(
            "function\tincrementer\t1:1:2\n",
            "  header\tpublic=false\tasync=false\tbody=true\treturn=Unit\n",
            "  stmt\tlet\tincrement\t\tclosure\tclosure\n",
            "  stmt\treturn\t\t\tliteral\tUnit\n",
        )
    );
}

#[test]
fn selfhost_braced_pipe_closure_body_is_materialized() {
    let source = r#"fn incrementer() -> Unit {
    let increment = |value| {
        let next = value + 1
        next
    }
    return Unit
}
"#;
    let exe = compile_selfhost_tool("serialize/outline.rss", "braced pipe closure AST outline")
        .expect("braced pipe closure AST outline should compile");
    let output = exe
        .eval_main_with_args([source.to_string()])
        .expect("braced pipe closure AST outline should run");
    assert_eq!(
        output.stdout,
        concat!(
            "function\tincrementer\t1:1:2\n",
            "  header\tpublic=false\tasync=false\tbody=true\treturn=Unit\n",
            "  stmt\tlet\tincrement\t\tclosure\tclosure\tclosure-body=2\n",
            "  stmt\treturn\t\t\tliteral\tUnit\n",
        )
    );
}

#[test]
fn selfhost_ast_outline_names_shared_expression_kinds() {
    let source = r#"fn values(image: Image) -> List<Int> {
    let shared = manage image
    let items = [1, 2]
    return items
}
"#;
    let exe = compile_selfhost_tool("serialize/outline.rss", "shared expression AST outline")
        .expect("shared expression AST outline should compile");
    let output = exe
        .eval_main_with_args([source.to_string()])
        .expect("shared expression AST outline should run");
    assert_eq!(
        output.stdout,
        concat!(
            "function\tvalues\t1:1:2\n",
            "  header\tpublic=false\tasync=false\tbody=true\treturn=List<Int>\n",
            "  param\timage\tImage\n",
            "  stmt\tlet\tshared\t\tmanage\tmanage\n",
            "  stmt\tlet\titems\t\tarray\tarray\n",
            "  stmt\treturn\t\t\tname\titems\n",
        )
    );
}

#[test]
fn selfhost_match_ast_outline_is_deterministic() {
    let source = r#"fn choose(value: read Option<Int>) -> Unit {
    match value {
        Some(item) => return Unit
        None => return Unit
    }
    return Unit
}
"#;
    let exe = compile_selfhost_tool("serialize/outline.rss", "match AST outline")
        .expect("match AST outline should compile");
    let output = exe
        .eval_main_with_args([source.to_string()])
        .expect("match AST outline should run");
    assert_eq!(
        output.stdout,
        concat!(
            "function\tchoose\t1:1:2\n",
            "  header\tpublic=false\tasync=false\tbody=true\treturn=Unit\n",
            "  param\tvalue\tread\tOption<Int>\n",
            "  stmt\tmatch\t\t\tname\tvalue\tarms=2\n",
            "    arm\treturn\tUnit\tbody=1\tpattern=variant\n",
            "    arm\treturn\tUnit\tbody=1\tpattern=variant\n",
            "  stmt\treturn\t\t\tliteral\tUnit\n",
        )
    );
}

#[test]
fn selfhost_match_arm_block_ast_outline_is_deterministic() {
    let source = r#"fn choose(value: read Option<Int>) -> Unit {
    match value {
        Some(item) => {
            let next: Int = item
            return Unit
        }
        None => "none"
    }
}
"#;
    let exe = compile_selfhost_tool("serialize/outline.rss", "match arm body AST outline")
        .expect("match arm body AST outline should compile");
    let output = exe
        .eval_main_with_args([source.to_string()])
        .expect("match arm body AST outline should run");
    assert_eq!(
        output.stdout,
        concat!(
            "function\tchoose\t1:1:2\n",
            "  header\tpublic=false\tasync=false\tbody=true\treturn=Unit\n",
            "  param\tvalue\tread\tOption<Int>\n",
            "  stmt\tmatch\t\t\tname\tvalue\tarms=2\n",
            "    arm\tlet\titem\tbody=2\tpattern=variant\n",
            "    arm\texpr\tnone\tbody=1\tpattern=variant\n",
        )
    );
}

#[test]
fn selfhost_match_expression_ast_outline_is_deterministic() {
    let source = r#"fn choose(value: read Option<Int>) -> Unit {
    let answer: Int = match value {
        Some(item) => item
        None => 0
    }
    return Unit
}
"#;
    let exe = compile_selfhost_tool("serialize/outline.rss", "match expression AST outline")
        .expect("match expression AST outline should compile");
    let output = exe
        .eval_main_with_args([source.to_string()])
        .expect("match expression AST outline should run");
    assert_eq!(
        output.stdout,
        concat!(
            "function\tchoose\t1:1:2\n",
            "  header\tpublic=false\tasync=false\tbody=true\treturn=Unit\n",
            "  param\tvalue\tread\tOption<Int>\n",
            "  stmt\tlet\tanswer\tInt\tmatch\tmatch\tarms=2\tguards=0\n",
            "  stmt\treturn\t\t\tliteral\tUnit\n",
        )
    );
}

#[test]
fn selfhost_match_expression_arm_type_oracle_anchor() {
    let source = r#"fn choose(value: read Option<Int>) -> Int {
    return match value {
        Some(item) => item
        None => "none"
    }
}
"#;
    let oracle = checker_oracle_records("match-expression-arm-type.rss", source, "RS0209");
    assert_eq!(
        oracle,
        vec![SelfhostDiagnosticRecord {
            code: "RS0209".to_string(),
            line: 4,
            column: 9,
            length: 4,
        }],
        "fixture must pin the incompatible arm pattern anchor"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0209",
    );
    assert_eq!(oracle, actual, "match expression arm types diverged");
}

#[test]
fn selfhost_result_match_expression_arm_type_parity() {
    let source = r#"fn choose(value: read Result<Int, String>) -> Int {
    return match value {
        Ok(item) => item
        Err(reason) => reason
    }
}
"#;
    let oracle = checker_oracle_records("result-match-expression-arm-type.rss", source, "RS0209");
    assert_eq!(
        oracle,
        vec![SelfhostDiagnosticRecord {
            code: "RS0209".to_string(),
            line: 4,
            column: 9,
            length: 3,
        }],
        "fixture must pin the Err arm pattern anchor"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0209",
    );
    assert_eq!(oracle, actual, "Result match expression arm types diverged");
}

#[test]
fn selfhost_match_guard_ast_outline_is_deterministic() {
    let source = r#"fn choose(value: read Option<Int>) -> Int {
    return match value {
        Some(item) if item == 1 => item
        None => 0
    }
}
"#;
    let exe = compile_selfhost_tool("serialize/outline.rss", "match guard AST outline")
        .expect("match guard AST outline should compile");
    let output = exe
        .eval_main_with_args([source.to_string()])
        .expect("match guard AST outline should run");
    assert_eq!(
        output.stdout,
        concat!(
            "function\tchoose\t1:1:2\n",
            "  header\tpublic=false\tasync=false\tbody=true\treturn=Int\n",
            "  param\tvalue\tread\tOption<Int>\n",
            "  stmt\treturn\t\t\tmatch\tmatch\tarms=2\tguards=1\n",
        )
    );
}

#[test]
fn selfhost_destructuring_pattern_ast_outline_is_deterministic() {
    let source = r#"fn choose(value: read Int) -> Unit {
    match value {
        (left, right) => return Unit
        [head, tail] => return Unit
    }
}
"#;
    let exe = compile_selfhost_tool("serialize/outline.rss", "destructuring pattern AST outline")
        .expect("destructuring pattern AST outline should compile");
    let output = exe
        .eval_main_with_args([source.to_string()])
        .expect("destructuring pattern AST outline should run");
    assert_eq!(
        output.stdout,
        concat!(
            "function\tchoose\t1:1:2\n",
            "  header\tpublic=false\tasync=false\tbody=true\treturn=Unit\n",
            "  param\tvalue\tread\tInt\n",
            "  stmt\tmatch\t\t\tname\tvalue\tarms=2\n",
            "    arm\treturn\tUnit\tbody=1\tpattern=tuple\tpattern-parts=2\n",
            "    arm\treturn\tUnit\tbody=1\tpattern=list\tpattern-parts=2\n",
        )
    );
}

#[test]
fn selfhost_with_statement_ast_outline_is_deterministic() {
    let source = r#"fn run(pool: read Pool) -> Unit {
    with pool.acquire() as resource {
        return Unit
    }
}
"#;
    let exe = compile_selfhost_tool("serialize/outline.rss", "with statement AST outline")
        .expect("with statement AST outline should compile");
    let output = exe
        .eval_main_with_args([source.to_string()])
        .expect("with statement AST outline should run");
    assert_eq!(
        output.stdout,
        concat!(
            "function\trun\t1:1:2\n",
            "  header\tpublic=false\tasync=false\tbody=true\treturn=Unit\n",
            "  param\tpool\tread\tPool\n",
            "  stmt\twith\tresource\t\tcall\tacquire\targs=0\tlabels=\tbody=1\n",
        )
    );
}

#[test]
fn selfhost_select_statement_ast_outline_is_deterministic() {
    let source = r#"fn pick(first: read Chan, second: read Chan) -> Unit {
    select {
        left = await first.receive() => { return Unit }
        right = await second.receive() => return Unit
    }
}
"#;
    let exe = compile_selfhost_tool("serialize/outline.rss", "select statement AST outline")
        .expect("select statement AST outline should compile");
    let output = exe
        .eval_main_with_args([source.to_string()])
        .expect("select statement AST outline should run");
    assert_eq!(
        output.stdout,
        concat!(
            "function\tpick\t1:1:2\n",
            "  header\tpublic=false\tasync=false\tbody=true\treturn=Unit\n",
            "  param\tfirst\tread\tChan\n",
            "  param\tsecond\tread\tChan\n",
            "  stmt\tselect\t\t\tnone\t\tarms=2\n",
            "    select-arm\tleft\tawait\tawait\tbody=1\n",
            "    select-arm\tright\tawait\tawait\tbody=1\n",
        )
    );
}

#[test]
fn selfhost_let_destructure_ast_outline_is_deterministic() {
    let source = r#"fn split(pair: read Pair) -> Unit {
    let (left, right) = pair
    return Unit
}
"#;
    let exe = compile_selfhost_tool("serialize/outline.rss", "let destructure AST outline")
        .expect("let destructure AST outline should compile");
    let output = exe
        .eval_main_with_args([source.to_string()])
        .expect("let destructure AST outline should run");
    assert_eq!(
        output.stdout,
        concat!(
            "function\tsplit\t1:1:2\n",
            "  header\tpublic=false\tasync=false\tbody=true\treturn=Unit\n",
            "  param\tpair\tread\tPair\n",
            "  stmt\tlet\t\t\tname\tpair\tdestructure=left,right\n",
            "  stmt\treturn\t\t\tliteral\tUnit\n",
        )
    );
}

#[test]
fn selfhost_function_context_infers_core_body_types() {
    let source = r#"fn consume(value: read Int) -> String {
    return "ok"
}

fn work(input: read Int) -> Unit {
    let number = 1
    let text: String = consume(value: input)
    if !(number == input) || false {
        return Unit
    }
    while false {
        return Unit
    }
    return Unit
}
"#;
    let exe = compile_selfhost_tool("serialize/type_outline.rss", "function-context probe")
        .expect("function-context probe should compile");
    let output = exe
        .eval_main_with_args([source.to_string()])
        .expect("function-context probe should run");
    assert_eq!(
        output.stdout,
        concat!(
            "consume\treturn\tString\n",
            "work\tlet\tInt\n",
            "work\tlet\tString\n",
            "work\tif\tBool\n",
            "work\twhile\tBool\n",
            "work\treturn\tUnit\n",
        )
    );
}

#[test]
fn selfhost_ast_control_type_rule_matches_rs0209_conditions() {
    let source = r#"fn conditions(name: read String, count: read Int) -> Unit {
    if name {
        return Unit
    }
    while count {
        return Unit
    }
    for item in name {
        Output.write(message: read "item")
    }
    if true {
        if count {
            return Unit
        }
    }
    return Unit
}
"#;
    let oracle = checker_oracle_records("ast-control-rs0209.rss", source, "RS0209");
    assert_eq!(
        oracle.len(),
        4,
        "fixture must exercise nested if/while and for subjects"
    );
    let exe = compile_selfhost_tool(
        "serialize/control_outline.rss",
        "AST control type-rule probe",
    )
    .expect("AST control type-rule probe should compile");
    let output = exe
        .eval_main_with_args([source.to_string()])
        .expect("AST control type-rule probe should run");
    let actual = parse_checker_records(&output.stdout)
        .expect("AST control type-rule probe should emit canonical records");
    assert_eq!(oracle, actual, "AST control type diagnostics diverged");
}

#[test]
fn selfhost_ast_match_pattern_rule_matches_rs0209_patterns() {
    let source = r#"fn patterns(value: read String) -> Unit {
    match value {
        Some(item) => return Unit
        1 => return Unit
        other => return Unit
    }
    return Unit
}
"#;
    let oracle = checker_oracle_records("ast-match-rs0209.rss", source, "RS0209");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must exercise variant, literal, and bare-name patterns"
    );
    let exe = compile_selfhost_tool("serialize/control_outline.rss", "AST match type-rule probe")
        .expect("AST match type-rule probe should compile");
    let output = exe
        .eval_main_with_args([source.to_string()])
        .expect("AST match type-rule probe should run");
    let actual = parse_checker_records(&output.stdout)
        .expect("AST match type-rule probe should emit canonical records");
    assert_eq!(oracle, actual, "AST match type diagnostics diverged");
}

#[test]
fn selfhost_ast_match_shape_rule_matches_rs0209_patterns() {
    let source = r#"fn tuple_pattern(value: read Int) -> Unit {
    match value {
        (left, right) => return Unit
    }
    return Unit
}

fn list_pattern(value: read String) -> Unit {
    match value {
        [item] => return Unit
    }
    return Unit
}
"#;
    let oracle = checker_oracle_records("ast-match-shape-rs0209.rss", source, "RS0209");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must exercise tuple and list pattern shapes"
    );
    let exe = compile_selfhost_tool(
        "serialize/control_outline.rss",
        "AST match shape-rule probe",
    )
    .expect("AST match shape-rule probe should compile");
    let output = exe
        .eval_main_with_args([source.to_string()])
        .expect("AST match shape-rule probe should run");
    let actual = parse_checker_records(&output.stdout)
        .expect("AST match shape-rule probe should emit canonical records");
    assert_eq!(oracle, actual, "AST match shape diagnostics diverged");
    let checker_actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0209",
    );
    assert_eq!(
        oracle, checker_actual,
        "main checker match shape diagnostics diverged"
    );
}

#[test]
fn selfhost_ast_type_rules_accept_deque_queue_kernel() {
    let source = include_str!("../../../../benchmarks/micro/deque_queue.rss");
    let exe = compile_selfhost_tool(
        "serialize/control_outline.rss",
        "AST deque queue type-rule probe",
    )
    .expect("AST deque queue type-rule probe should compile");
    let output = exe
        .eval_main_with_args([source.to_string()])
        .expect("AST deque queue type-rule probe should run");
    assert!(
        output.stdout.trim().is_empty(),
        "valid deque queue kernel must not produce AST type diagnostics: {:?}",
        output.stdout
    );
}

#[test]
fn selfhost_checker_accepts_diagnostic_ast_module() {
    let source = include_str!("../../../../selfhost/semantics/diagnostics.rss");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0208",
    );
    assert!(
        actual.is_empty(),
        "valid diagnostic AST constructors must not report return-type mismatches: {actual:?}"
    );
}

#[test]
fn selfhost_checker_accepts_mutable_handle_parameter_kernel() {
    let source =
        include_str!("../../../../benchmarks/vm-jit/kernels/native_call_mut_handle_param.rss");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0311",
    );
    assert!(
        actual.is_empty(),
        "mutating a mut Handle parameter field must not report immutable assignment: {actual:?}"
    );
}

#[test]
fn selfhost_call_binding_preserves_evaluation_and_parameter_order() {
    let source = r#"fn digits(a: Int = 1, b: Int, c: Int = 3) -> Int {
    return a * 100 + b * 10 + c
}

fn pair(first: Int, second: Int) -> Int {
    return first * 10 + second
}

class Box {
    value: Int
}

struct Defaults {
    first: Int = 1
    second: Int
}

sum Duo {
    Values(first: Int, second: Int)
}

fn Box.add(self: read Box, amount: Int = 1) -> Int {
    return self.value + amount
}

fn main() -> Unit {
    let first: Int = 1
    let second: Int = 2
    let box = Box(value: 4)
    let defaults = Defaults(second)
    let duo = Values(second, first)
    digits(c: 9, b: 2)
    pair(second, first)
    box.add()
    return Unit
}
"#;
    let exe = compile_selfhost_tool(
        "serialize/call_binding_outline.rss",
        "canonical call-binding outline",
    )
    .expect("canonical call-binding outline should compile");
    let output = exe
        .eval_main_with_args([source.to_string()])
        .expect("canonical call-binding outline should run");
    assert_eq!(
        output.stdout,
        concat!(
            "Box\tvalue\targ0\teval=0\n",
            "Box\tstatus\tcomplete\n",
            "Defaults\tfirst\tdefault\teval=1\n",
            "Defaults\tsecond\targ0\teval=0\n",
            "Defaults\tstatus\tcomplete\n",
            "Values\tfirst\targ1\teval=1\n",
            "Values\tsecond\targ0\teval=0\n",
            "Values\tstatus\tcomplete\n",
            "digits\ta\tdefault\teval=2\n",
            "digits\tb\targ1\teval=1\n",
            "digits\tc\targ0\teval=0\n",
            "digits\tstatus\tcomplete\n",
            "pair\tfirst\targ1\teval=1\n",
            "pair\tsecond\targ0\teval=0\n",
            "pair\tstatus\tcomplete\n",
            "add\tself\treceiver\teval=0\n",
            "add\tamount\tdefault\teval=1\n",
            "add\tstatus\tcomplete\n",
        )
    );
}

/// Phase-2 NEGATIVE smoke (non-ignored): the rss parser must REJECT malformed
/// source, matching the Rust oracle. The accept-only tiny sample above would
/// still pass if the rss parser degenerated to always printing `OK`; this closes
/// that gap without needing the (ignored) full-corpus gate.
#[test]
fn parser_rejects_malformed_source_smoke() {
    let source = "fn main() -> Unit {\n    return Unit\n}\n\nfn\n";
    let oracle = parse_oracle_error("parser-negative.rss", source);
    assert!(
        oracle.is_some(),
        "oracle Rust parser must reject the malformed sample (else the smoke test proves nothing)"
    );
    let exe = compile_parser().expect("rss parser should compile");
    let actual = run_parser(&exe, source).expect("rss parser should run");
    // Recognition tier: both must reject (accept-vs-reject only).
    compare_parse(oracle, actual, false).unwrap_or_else(|msg| panic!("{msg}"));
}

/// Phase-2 gate (ignored by default): the rss parser's accept/reject matches the
/// Rust parser over the whole `.rss` corpus.
#[test]
#[ignore]
fn parser_parity_corpus() {
    let root = workspace_root();
    let files = collect_rss_files(&root).expect("corpus discovery should succeed");
    let position = parse_position_tier();
    let exe = compile_parser().expect("rss parser should compile");
    let mut run_failures: Vec<String> = Vec::new();
    let mut mismatches: Vec<String> = Vec::new();
    let mut ok = 0usize;
    for file in &files {
        let rel = file
            .strip_prefix(&root)
            .unwrap_or(file)
            .display()
            .to_string();
        let source =
            std::fs::read_to_string(file).unwrap_or_else(|e| panic!("cannot read {rel}: {e}"));
        let oracle = parse_oracle_error(&rel, &source);
        match run_parser(&exe, &source) {
            Err(e) => run_failures.push(format!("{rel}: {e}")),
            Ok(actual) => match compare_parse(oracle, actual, position) {
                Ok(()) => ok += 1,
                Err(msg) => mismatches.push(format!("{rel}: {msg}")),
            },
        }
    }
    let total = files.len();
    eprintln!(
        "\n=== parser_parity_corpus (position={position}) ===\n  files: {total}\n  ok: {ok}\n  \
         run-failures: {}\n  verdict-mismatches: {}\n",
        run_failures.len(),
        mismatches.len()
    );
    for line in run_failures.iter().take(20) {
        eprintln!("[run-fail] {line}");
    }
    for line in mismatches.iter().take(20) {
        eprintln!("[mismatch] {line}");
    }
    assert!(
        run_failures.is_empty() && mismatches.is_empty(),
        "parser parity failed: {} run-failures, {} mismatches (of {total})",
        run_failures.len(),
        mismatches.len()
    );
}
