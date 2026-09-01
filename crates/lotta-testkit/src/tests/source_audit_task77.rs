use proc_macro2::Span;
use quote::ToTokens as _;
use std::path::{Path, PathBuf};
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

fn findings(source: &str) -> Result<Vec<Finding>, syn::Error> {
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
    let file = syn::parse_file(source)?;
    let mut visitor = FunctionVisitor::default();
    visitor.visit_file(&file);
    result.extend(visitor.findings);
    Ok(result)
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
    let mut audited = 0_usize;
    for relative in TASK77_RUST_SOURCES {
        let path = root.join(relative);
        let source = std::fs::read_to_string(&path).expect("read Task77 source");
        let violations = findings(&source).expect("parse Task77 source");
        assert!(
            violations.is_empty(),
            "Task77 source {}: {violations:?}",
            path.display()
        );
        audited += 1;
    }
    assert_eq!(audited, TASK77_RUST_SOURCES.len(), "audit manifest gap");
}

#[test]
fn file_lines_mutation_is_rejected() {
    let mutation = "\n".repeat(FILE_LINES_MAX + 1);
    let violations = findings(&mutation).expect("parse FileLines mutation");
    assert!(
        violations
            .iter()
            .any(|finding| matches!(finding, Finding::FileLines(1_001))),
        "oversized audited file escaped: {violations:?}"
    );
}
