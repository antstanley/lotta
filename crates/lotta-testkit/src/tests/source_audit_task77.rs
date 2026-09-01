use proc_macro2::Span;
use quote::ToTokens as _;
use std::{
    io::Read as _,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};
use syn::{
    Attribute, ImplItem, Item, TraitItem,
    spanned::Spanned as _,
    visit::{self, Visit},
};

const FILE_LINES_MAX: usize = 1_000;
const FUNCTION_LINES_MAX: usize = 70;
const LINE_BYTES_MAX: usize = 100;
const TASK77_RUST_SOURCES: &[&str] = &[
    "crates/lotta-app-server/src/openai/chat.rs",
    "crates/lotta-app-server/src/openai/chat/render.rs",
    "tests/openai_golden.rs",
    "tests/openai_golden/cases.rs",
    "tests/openai_golden/comparator.rs",
    "tests/openai_golden/harness.rs",
    "tests/openai_golden/normalize.rs",
    "tests/openai_golden/sanitize.rs",
    "tests/openai_golden/schema.rs",
    "crates/lotta-testkit/src/tests/source_audit_task77.rs",
];
const TASK77_SCRIPT_SOURCES: &[&str] = &[
    "tools/capture-openai-fixtures.mjs",
    "tools/openai-capture-adapter.ts",
    "tools/openai-capture-runner.ts",
];

#[derive(Debug, Eq, PartialEq)]
enum Finding {
    FileLines(usize),
    LineBytes { line: usize, bytes: usize },
    FunctionLines { name: String, lines: usize },
}

#[derive(Default)]
struct FunctionVisitor {
    findings: Vec<Finding>,
}

impl FunctionVisitor {
    fn check(&mut self, name: &str, span: Span, attributes: &[Attribute]) {
        if cfg_test(attributes) {
            return;
        }
        let lines = span.end().line.saturating_sub(span.start().line) + 1;
        if lines > FUNCTION_LINES_MAX {
            self.findings.push(Finding::FunctionLines {
                name: name.to_owned(),
                lines,
            });
        }
    }
}

impl<'ast> Visit<'ast> for FunctionVisitor {
    fn visit_item(&mut self, item: &'ast Item) {
        if let Item::Fn(function) = item {
            self.check(
                &function.sig.ident.to_string(),
                function.span(),
                &function.attrs,
            );
        }
        visit::visit_item(self, item);
    }

    fn visit_impl_item(&mut self, item: &'ast ImplItem) {
        if let ImplItem::Fn(function) = item {
            self.check(
                &function.sig.ident.to_string(),
                function.span(),
                &function.attrs,
            );
        }
        visit::visit_impl_item(self, item);
    }

    fn visit_trait_item(&mut self, item: &'ast TraitItem) {
        if let TraitItem::Fn(function) = item
            && function.default.is_some()
        {
            self.check(
                &function.sig.ident.to_string(),
                function.span(),
                &function.attrs,
            );
        }
        visit::visit_trait_item(self, item);
    }
}

fn cfg_test(attributes: &[Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        attribute.path().is_ident("test")
            || (attribute.path().is_ident("cfg")
                && attribute
                    .meta
                    .to_token_stream()
                    .to_string()
                    .contains("test"))
    })
}

fn common_findings(source: &str) -> Vec<Finding> {
    let mut result = Vec::new();
    let lines = source.lines().count();
    if lines > FILE_LINES_MAX {
        result.push(Finding::FileLines(lines));
    }
    for (index, line) in source.lines().enumerate() {
        if line.len() > LINE_BYTES_MAX {
            result.push(Finding::LineBytes {
                line: index + 1,
                bytes: line.len(),
            });
        }
    }
    result
}

fn rust_findings(source: &str) -> Result<Vec<Finding>, syn::Error> {
    let mut result = common_findings(source);
    let file = syn::parse_file(source)?;
    let mut visitor = FunctionVisitor::default();
    visitor.visit_file(&file);
    result.extend(visitor.findings);
    Ok(result)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ScriptMode {
    Code,
    Single,
    Double,
    Template,
    BlockComment,
}

#[derive(Debug)]
struct ScriptState {
    mode: ScriptMode,
    escaped: bool,
    delimiters: Vec<(char, usize)>,
    functions: Vec<(usize, usize)>,
    findings: Vec<Finding>,
}

impl Default for ScriptState {
    fn default() -> Self {
        Self {
            mode: ScriptMode::Code,
            escaped: false,
            delimiters: Vec::new(),
            functions: Vec::new(),
            findings: Vec::new(),
        }
    }
}

fn script_findings(source: &str) -> Vec<Finding> {
    let mut state = ScriptState::default();
    for (index, line) in source.lines().enumerate() {
        scan_script_line(line, index + 1, &mut state);
    }
    finish_script(source.lines().count(), &mut state);
    let mut result = common_findings(source);
    result.extend(state.findings);
    result
}

fn scan_script_line(line: &str, number: usize, state: &mut ScriptState) {
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if consume_script_mode(bytes, &mut index, state) {
            continue;
        }
        if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'/') {
            break;
        }
        if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'*') {
            state.mode = ScriptMode::BlockComment;
            index += 2;
            continue;
        }
        scan_code_byte(line, bytes[index], number, index, state);
        index += 1;
    }
}

fn consume_script_mode(bytes: &[u8], index: &mut usize, state: &mut ScriptState) -> bool {
    if state.mode == ScriptMode::Code {
        return false;
    }
    let byte = bytes[*index];
    if state.mode == ScriptMode::BlockComment {
        if byte == b'*' && bytes.get(*index + 1) == Some(&b'/') {
            state.mode = ScriptMode::Code;
            *index += 2;
        } else {
            *index += 1;
        }
        return true;
    }
    if state.escaped {
        state.escaped = false;
    } else if byte == b'\\' {
        state.escaped = true;
    } else if closes_mode(byte, state.mode) {
        state.mode = ScriptMode::Code;
    }
    *index += 1;
    true
}

fn closes_mode(byte: u8, mode: ScriptMode) -> bool {
    matches!(
        (byte, mode),
        (b'\'', ScriptMode::Single) | (b'"', ScriptMode::Double) | (b'`', ScriptMode::Template)
    )
}

fn scan_code_byte(line: &str, byte: u8, number: usize, column: usize, state: &mut ScriptState) {
    match byte {
        b'\'' => state.mode = ScriptMode::Single,
        b'"' => state.mode = ScriptMode::Double,
        b'`' => state.mode = ScriptMode::Template,
        b'(' | b'[' | b'{' => open_delimiter(line, byte as char, number, column, state),
        b')' | b']' | b'}' => close_delimiter(byte as char, number, state),
        _ => {}
    }
}

fn open_delimiter(
    line: &str,
    delimiter: char,
    number: usize,
    column: usize,
    state: &mut ScriptState,
) {
    state.delimiters.push((delimiter, number));
    if delimiter == '{' && function_open(&line[..column]) {
        state.functions.push((state.delimiters.len(), number));
    }
}

fn function_open(prefix: &str) -> bool {
    let text = prefix.trim_end();
    text.ends_with("=>")
        || text.contains("function ")
        || text.ends_with(')')
        || text.starts_with("get ")
        || text.starts_with("set ")
}

fn close_delimiter(delimiter: char, line: usize, state: &mut ScriptState) {
    let expected = match delimiter {
        ')' => '(',
        ']' => '[',
        '}' => '{',
        _ => return,
    };
    let depth = state.delimiters.len();
    if state
        .delimiters
        .last()
        .is_some_and(|(actual, _)| *actual == expected)
    {
        state.delimiters.pop();
    }
    if delimiter == '}' {
        close_function(depth, line, state);
    }
}

fn close_function(depth: usize, line: usize, state: &mut ScriptState) {
    let Some(index) = state
        .functions
        .iter()
        .rposition(|(function_depth, _)| *function_depth == depth)
    else {
        return;
    };
    let (_, start) = state.functions.remove(index);
    let lines = line.saturating_sub(start) + 1;
    if lines > FUNCTION_LINES_MAX {
        state.findings.push(Finding::FunctionLines {
            name: format!("script@{start}"),
            lines,
        });
    }
}

fn finish_script(_last_line: usize, state: &mut ScriptState) {
    state.delimiters.clear();
}

const BUN_VERSION: &str = "1.3.14";
const BUN_DEADLINE: Duration = Duration::from_secs(15);
const BUN_PARSE: &str = r"const p=process.argv[1];
const loader=p.endsWith('.ts')?'ts':'js';
new Bun.Transpiler({loader}).transformSync(await Bun.file(p).text());";

fn bun_command() -> String {
    std::env::var("LOTTA_BUN").unwrap_or_else(|_| "bun".to_owned())
}

fn bounded_status(mut child: Child) -> Result<(ExitStatus, String), String> {
    let start = Instant::now();
    loop {
        match child.try_wait().map_err(|error| error.to_string())? {
            Some(status) => {
                let mut stderr = String::new();
                if let Some(mut pipe) = child.stderr.take() {
                    pipe.read_to_string(&mut stderr)
                        .map_err(|error| error.to_string())?;
                }
                return Ok((status, stderr));
            }
            None if start.elapsed() < BUN_DEADLINE => {
                thread::sleep(Duration::from_millis(10));
            }
            None => {
                child.kill().map_err(|error| error.to_string())?;
                child.wait().map_err(|error| error.to_string())?;
                return Err("Bun parser deadline exceeded".to_owned());
            }
        }
    }
}

fn run_bun(arguments: &[&str]) -> Result<(), String> {
    let child = Command::new(bun_command())
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Bun unavailable: {error}"))?;
    let (status, stderr) = bounded_status(child)?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("Bun parser failed: {stderr}"))
    }
}

fn assert_pinned_bun() {
    let check = format!("if(Bun.version!=={BUN_VERSION:?})throw Error('Bun '+Bun.version)");
    run_bun(&["-e", &check]).expect("Task77 requires pinned Bun parser");
}

fn parse_script(path: &Path) -> Result<(), String> {
    let text = path
        .to_str()
        .ok_or_else(|| "non-UTF8 script path".to_owned())?;
    run_bun(&["-e", BUN_PARSE, text])
}

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

#[test]
fn task77_sources_obey_hard_limits() {
    let root = workspace();
    for relative in TASK77_RUST_SOURCES {
        let path = root.join(relative);
        let source = std::fs::read_to_string(&path).expect("read Task77 Rust source");
        let violations = rust_findings(&source).expect("parse Task77 Rust source");
        assert!(violations.is_empty(), "{}: {violations:?}", path.display());
    }
    assert_pinned_bun();
    let mut parsed = Vec::new();
    for relative in TASK77_SCRIPT_SOURCES {
        let path = root.join(relative);
        let source = std::fs::read_to_string(&path).expect("read Task77 script source");
        let violations = script_findings(&source);
        assert!(violations.is_empty(), "{}: {violations:?}", path.display());
        parse_script(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        parsed.push(*relative);
    }
    assert_eq!(
        parsed, TASK77_SCRIPT_SOURCES,
        "exact capture sources parsed"
    );
}

#[test]
fn audit_manifest_has_no_gap() {
    assert_eq!(TASK77_RUST_SOURCES.len(), 10);
    assert_eq!(TASK77_SCRIPT_SOURCES.len(), 3);
}

#[test]
fn file_line_function_and_syntax_mutations_are_rejected() {
    let file = "\n".repeat(FILE_LINES_MAX + 1);
    assert!(common_findings(&file).contains(&Finding::FileLines(1_001)));
    let long_line = "x".repeat(LINE_BYTES_MAX + 1);
    assert!(matches!(
        common_findings(&long_line).as_slice(),
        [Finding::LineBytes {
            line: 1,
            bytes: 101
        }]
    ));
    let function = format!("function oversized() {{\n{}\n}}", "x;\n".repeat(70));
    assert!(
        script_findings(&function)
            .iter()
            .any(|finding| matches!(finding, Finding::FunctionLines { .. }))
    );
    assert_pinned_bun();
    let mutation = workspace()
        .join("target")
        .join(format!("task77-balanced-syntax-{}.ts", std::process::id()));
    std::fs::write(&mutation, "const value = ();\nconst broken = ;\n")
        .expect("write syntax mutation");
    let failure = parse_script(&mutation);
    std::fs::remove_file(&mutation).expect("remove syntax mutation");
    assert!(
        failure.is_err(),
        "balanced-delimiter invalid syntax escaped"
    );
}
