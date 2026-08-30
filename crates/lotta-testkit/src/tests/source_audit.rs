use quote::ToTokens;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{Attribute, ExprCall, ExprMethodCall, ImplItem, Item, Macro, TraitItem, UseTree};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum Effect {
    WallTime,
    Sleep,
    Network,
    Process,
    Entropy,
    Filesystem,
    Include,
    GeneratedSource,
}

#[derive(Debug, Eq, PartialEq)]
struct Finding {
    effect: Effect,
    path: String,
}

#[derive(Default)]
struct Analyzer {
    scopes: Vec<BTreeMap<String, String>>,
    findings: Vec<Finding>,
    filesystem_allowed: bool,
}

impl Analyzer {
    fn analyze(source: &str, filesystem_allowed: bool) -> Result<Vec<Finding>, syn::Error> {
        let file = syn::parse_file(source)?;
        let mut analyzer = Self {
            scopes: vec![BTreeMap::new()],
            filesystem_allowed,
            ..Self::default()
        };
        analyzer.visit_file(&file);
        Ok(analyzer.findings)
    }

    fn resolve(&self, path: String) -> String {
        let (first, rest) = path
            .split_once("::")
            .map_or((path.as_str(), None), |(a, b)| (a, Some(b)));
        for scope in self.scopes.iter().rev() {
            if let Some(base) = scope.get(first) {
                return rest.map_or_else(|| base.clone(), |tail| format!("{base}::{tail}"));
            }
        }
        path
    }

    fn record_path(&mut self, path: &syn::Path) {
        let path = self.resolve(path_string(path));
        if let Some(effect) = classify_path(&path, self.filesystem_allowed) {
            self.findings.push(Finding { effect, path });
        }
    }

    fn with_scope(&mut self, run: impl FnOnce(&mut Self)) {
        self.scopes.push(BTreeMap::new());
        run(self);
        self.scopes.pop();
    }
}

impl<'ast> Visit<'ast> for Analyzer {
    fn visit_item(&mut self, item: &'ast Item) {
        if cfg_test(item_attrs(item))
            || matches!(item, Item::Mod(module) if module.ident == "tests")
        {
            return;
        }
        if let Item::Use(item_use) = item {
            collect_use(
                &item_use.tree,
                "",
                self.scopes.last_mut().expect("scope"),
                &mut self.findings,
            );
            return;
        }
        match item {
            Item::Fn(function) => self.with_scope(|this| visit::visit_item_fn(this, function)),
            Item::Mod(module) => self.with_scope(|this| visit::visit_item_mod(this, module)),
            _ => visit::visit_item(self, item),
        }
    }

    fn visit_block(&mut self, block: &'ast syn::Block) {
        self.with_scope(|this| visit::visit_block(this, block));
    }

    fn visit_stmt(&mut self, statement: &'ast syn::Stmt) {
        if let syn::Stmt::Item(Item::Use(item_use)) = statement {
            if !cfg_test(&item_use.attrs) {
                collect_use(
                    &item_use.tree,
                    "",
                    self.scopes.last_mut().expect("scope"),
                    &mut self.findings,
                );
            }
            return;
        }
        visit::visit_stmt(self, statement);
    }

    fn visit_expr_call(&mut self, call: &'ast ExprCall) {
        if let syn::Expr::Path(path) = call.func.as_ref() {
            self.record_path(&path.path);
        }
        visit::visit_expr_call(self, call);
    }

    fn visit_expr_method_call(&mut self, call: &'ast ExprMethodCall) {
        if let syn::Expr::Path(receiver) = call.receiver.as_ref() {
            let receiver = self.resolve(path_string(&receiver.path));
            let path = format!("{receiver}::{}", call.method);
            if let Some(effect) = classify_path(&path, self.filesystem_allowed) {
                self.findings.push(Finding { effect, path });
            }
        }
        visit::visit_expr_method_call(self, call);
    }

    fn visit_macro(&mut self, mac: &'ast Macro) {
        let path = self.resolve(path_string(&mac.path));
        let effect = match path.rsplit("::").next() {
            Some("include") => Some(Effect::GeneratedSource),
            Some("include_bytes" | "include_str") if !self.filesystem_allowed => {
                Some(Effect::Include)
            }
            _ => classify_path(&path, self.filesystem_allowed),
        };
        if let Some(effect) = effect {
            self.findings.push(Finding { effect, path });
        }
        visit::visit_macro(self, mac);
    }
}

fn classify_path(path: &str, filesystem_allowed: bool) -> Option<Effect> {
    let leaf = path.rsplit("::").next().unwrap_or(path);
    if (leaf == "now" && contains_any(path, &["SystemTime", "Utc", "Local", "Instant"]))
        || matches!(leaf, "timeout" | "sleep_until" | "interval" | "interval_at")
    {
        return Some(Effect::WallTime);
    }
    if leaf == "sleep" && contains_any(path, &["thread", "time", "Timer"]) {
        return Some(Effect::Sleep);
    }
    if contains_any(
        path,
        &[
            "TcpStream",
            "TcpListener",
            "UdpSocket",
            "ToSocketAddrs",
            "reqwest",
            "hyper",
            "lookup_host",
        ],
    ) && matches!(
        leaf,
        "new" | "connect" | "bind" | "to_socket_addrs" | "lookup_host"
    ) {
        return Some(Effect::Network);
    }
    if contains_any(
        path,
        &["process::Command", "Command::new", "Command::spawn"],
    ) {
        return Some(Effect::Process);
    }
    if contains_any(
        path,
        &[
            "rand::",
            "getrandom::",
            "Uuid::new_v4",
            "uuid::Uuid::new_v4",
        ],
    ) {
        return Some(Effect::Entropy);
    }
    if !filesystem_allowed
        && (path.starts_with("std::fs::")
            || filesystem_leaf(path)
            || contains_any(path, &["File::open", "File::create", "OpenOptions::new"]))
    {
        return Some(Effect::Filesystem);
    }
    None
}

fn filesystem_leaf(path: &str) -> bool {
    matches!(
        path,
        "read"
            | "read_to_string"
            | "write"
            | "remove_file"
            | "remove_dir"
            | "remove_dir_all"
            | "create_dir"
            | "create_dir_all"
            | "rename"
            | "copy"
            | "canonicalize"
            | "metadata"
            | "symlink_metadata"
            | "read_dir"
    )
}

fn contains_any(path: &str, values: &[&str]) -> bool {
    values.iter().any(|value| path.contains(value))
}

fn collect_use(
    tree: &UseTree,
    prefix: &str,
    aliases: &mut BTreeMap<String, String>,
    findings: &mut Vec<Finding>,
) {
    match tree {
        UseTree::Path(path) => collect_use(
            &path.tree,
            &join(prefix, &path.ident.to_string()),
            aliases,
            findings,
        ),
        UseTree::Name(name) => {
            aliases.insert(
                name.ident.to_string(),
                join(prefix, &name.ident.to_string()),
            );
        }
        UseTree::Rename(rename) => {
            aliases.insert(
                rename.rename.to_string(),
                join(prefix, &rename.ident.to_string()),
            );
        }
        UseTree::Group(group) => {
            for item in &group.items {
                collect_use(item, prefix, aliases, findings);
            }
        }
        UseTree::Glob(_) => {
            if prefix.starts_with("std::fs") {
                findings.push(Finding {
                    effect: Effect::Filesystem,
                    path: format!("{prefix}::*"),
                });
            }
        }
    }
}

fn join(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.into()
    } else {
        format!("{prefix}::{name}")
    }
}
fn path_string(path: &syn::Path) -> String {
    path.segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}
fn cfg_test(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("test")
            || (a.path().is_ident("cfg") && a.meta.to_token_stream().to_string().contains("test"))
    })
}

fn item_attrs(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(x) => &x.attrs,
        Item::Enum(x) => &x.attrs,
        Item::ExternCrate(x) => &x.attrs,
        Item::Fn(x) => &x.attrs,
        Item::ForeignMod(x) => &x.attrs,
        Item::Impl(x) => &x.attrs,
        Item::Macro(x) => &x.attrs,
        Item::Mod(x) => &x.attrs,
        Item::Static(x) => &x.attrs,
        Item::Struct(x) => &x.attrs,
        Item::Trait(x) => &x.attrs,
        Item::TraitAlias(x) => &x.attrs,
        Item::Type(x) => &x.attrs,
        Item::Union(x) => &x.attrs,
        Item::Use(x) => &x.attrs,
        _ => &[],
    }
}

#[derive(Debug, Eq, PartialEq)]
enum LimitFinding {
    FileLines(usize),
    LineWidth { line: usize, bytes: usize },
    FunctionLines { name: String, lines: usize },
    AllowSuppression { line: usize },
}

#[derive(Default)]
struct LimitVisitor {
    findings: Vec<LimitFinding>,
}

impl LimitVisitor {
    fn function(&mut self, name: &str, span: proc_macro2::Span, attrs: &[Attribute]) {
        if cfg_test(attrs) {
            return;
        }
        let lines = span.end().line.saturating_sub(span.start().line) + 1;
        if lines > 70 {
            self.findings.push(LimitFinding::FunctionLines {
                name: name.into(),
                lines,
            });
        }
    }

    fn suppressions(&mut self, attrs: &[Attribute]) {
        for attr in attrs {
            if attr.path().is_ident("allow") {
                self.findings.push(LimitFinding::AllowSuppression {
                    line: attr.span().start().line,
                });
            }
        }
    }
}

impl<'ast> Visit<'ast> for LimitVisitor {
    fn visit_item(&mut self, item: &'ast Item) {
        let attrs = item_attrs(item);
        if cfg_test(attrs) || matches!(item, Item::Mod(module) if module.ident == "tests") {
            return;
        }
        self.suppressions(attrs);
        if let Item::Fn(function) = item {
            self.function(
                &function.sig.ident.to_string(),
                function.span(),
                &function.attrs,
            );
        }
        visit::visit_item(self, item);
    }

    fn visit_impl_item(&mut self, item: &'ast ImplItem) {
        if let ImplItem::Fn(function) = item {
            if cfg_test(&function.attrs) {
                return;
            }
            self.suppressions(&function.attrs);
            self.function(
                &function.sig.ident.to_string(),
                function.span(),
                &function.attrs,
            );
        }
        visit::visit_impl_item(self, item);
    }

    fn visit_trait_item(&mut self, item: &'ast TraitItem) {
        if let TraitItem::Fn(function) = item {
            if cfg_test(&function.attrs) {
                return;
            }
            self.suppressions(&function.attrs);
            if function.default.is_some() {
                self.function(
                    &function.sig.ident.to_string(),
                    function.span(),
                    &function.attrs,
                );
            }
        }
        visit::visit_trait_item(self, item);
    }
}

fn limit_findings(source: &str) -> Result<Vec<LimitFinding>, syn::Error> {
    let mut findings = Vec::new();
    let line_count = source.lines().count();
    if line_count > 1_000 {
        findings.push(LimitFinding::FileLines(line_count));
    }
    for (index, line) in source.lines().enumerate() {
        if line.len() > 100 {
            findings.push(LimitFinding::LineWidth {
                line: index + 1,
                bytes: line.len(),
            });
        }
    }
    let file = syn::parse_file(source)?;
    let mut visitor = LimitVisitor::default();
    visitor.visit_file(&file);
    findings.extend(visitor.findings);
    Ok(findings)
}

fn rust_sources(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory)? {
            let path = entry?.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn production_rust_sources(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    rust_sources(root).map(|paths| {
        paths
            .into_iter()
            .filter(|path| !is_test_source(path))
            .collect()
    })
}

fn is_test_source(path: &Path) -> bool {
    if path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .any(|component| component == "tests" || component.ends_with("_tests"))
    {
        return true;
    }
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| {
            stem == "tests"
                || stem == "test_support"
                || stem.contains("_test")
                || stem.ends_with("_tests")
                || stem.ends_with("_certificate")
                || stem.ends_with("_evidence")
                || matches!(
                    stem,
                    "stop_reasons" | "terminal_once" | "tool_call_assembly"
                )
        })
}

fn hard_limit_sources(workspace: &Path, manifest: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files = production_rust_sources(&manifest.join("src"))?;
    for relative in [
        "src/production_components.rs",
        "crates/lotta-app-server/src/listener.rs",
        "crates/lotta-app-server/src/listener/turn_supervisor.rs",
        "crates/lotta-app-server/src/ws/connection.rs",
        "crates/lotta-app-server/src/ws/event.rs",
        "crates/lotta-app-server/src/ws/recovery.rs",
        "crates/lotta-app-server/src/ws/router.rs",
        "crates/lotta-app-server/src/ws/service.rs",
        "crates/lotta-app-server/src/ws/sync.rs",
        "crates/lotta-runtime/src/approval/mod.rs",
        "crates/lotta-runtime/src/approval/recovery.rs",
        "crates/lotta-runtime/src/approval/request.rs",
        "crates/lotta-runtime/src/approval/resolve.rs",
        "crates/lotta-runtime/src/registry.rs",
        "crates/lotta-store/src/approval.rs",
    ] {
        files.push(workspace.join(relative));
    }
    files.sort();
    files.dedup();
    Ok(files)
}

#[test]
fn source_hard_limits_and_mutations() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");
    let files = hard_limit_sources(workspace, &manifest).expect("discover production Rust sources");
    let mut source_findings = Vec::new();
    for path in files {
        let source = std::fs::read_to_string(&path).expect("read Rust source");
        let findings = limit_findings(&source).expect("parse Rust source");
        let findings: Vec<_> = findings
            .into_iter()
            .filter(|finding| !matches!(finding, LimitFinding::FileLines(_)))
            .collect();
        if !findings.is_empty() {
            source_findings.push(format!("{}: {findings:?}", path.display()));
        }
    }
    assert!(source_findings.is_empty(), "{source_findings:#?}");
    let long_file = "\n".repeat(1_001);
    let long_line = format!("const X: &str = \"{}\";", "x".repeat(101));
    let long_function = format!("fn oversized() {{\n{}\n}}", "let _x = 1;\n".repeat(70));
    let mutations = [
        (long_file, "file"),
        (long_line, "line"),
        (long_function, "function"),
        ("#[allow(dead_code)] fn x() {}".into(), "suppression"),
    ];
    for (source, label) in mutations {
        assert!(
            !limit_findings(&source).expect("parse mutation").is_empty(),
            "{label} mutation escaped hard-limit scanner"
        );
    }
    assert!(
        limit_findings("#[cfg(test)] #[allow(dead_code)] fn test_only() {}")
            .expect("parse test-only mutation")
            .is_empty()
    );
}

#[test]
fn source_audit_structural_production_scan() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let files = rust_sources(&root).expect("discover Rust sources");
    assert!(files.len() >= 10);
    for path in files {
        if path
            .components()
            .any(|component| component.as_os_str() == "tests")
        {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("read Rust source");
        let allowed = matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("fixtures.rs" | "roots.rs")
        );
        let findings = Analyzer::analyze(&source, allowed).expect("parse production source");
        assert!(findings.is_empty(), "{}: {findings:?}", path.display());
    }
}

#[test]
fn source_audit_mutation_matrix() {
    let mutations = [
        (Effect::GeneratedSource, "include!(\"generated.rs\");"),
        (Effect::Include, "const X:&[u8]=include_bytes!(\"x\");"),
        (Effect::Include, "const X:&str=include_str!(\"x\");"),
        (
            Effect::Filesystem,
            "fn x(){use std::{fs as disk}; disk::read(\"x\");}",
        ),
        (
            Effect::Filesystem,
            "fn x(){use std::fs::{read as ingest}; ingest(\"x\");}",
        ),
        (Effect::Filesystem, "fn x(){use std::fs::*; read(\"x\");}"),
        (
            Effect::WallTime,
            "async fn x(){use tokio::{time::{timeout as t}}; t(Default::default(),async{}).await;}",
        ),
        (
            Effect::Sleep,
            "fn x(){use std::thread::sleep as pause; pause(Default::default());}",
        ),
        (
            Effect::Network,
            "fn x(){use std::net::TcpStream as Wire; Wire::connect(\"x\");}",
        ),
        (
            Effect::Process,
            "fn x(){use std::process::Command as Exec; Exec::new(\"x\");}",
        ),
        (
            Effect::Entropy,
            "fn x(){use rand::random as choose; choose::<u8>();}",
        ),
        (
            Effect::WallTime,
            "fn x(){ std :: time :: SystemTime :: now ();}",
        ),
        (
            Effect::Filesystem,
            "fn x(){ std :: fs :: OpenOptions :: new ();}",
        ),
    ];
    for (effect, source) in mutations {
        let findings = Analyzer::analyze(source, false).expect("parse mutation");
        assert!(
            findings.iter().any(|finding| finding.effect == effect),
            "{effect:?}: {source}: {findings:?}"
        );
    }
    for source in [
        "fn x(){object.connect();}",
        "fn x(){object.bind();}",
        "fn x(){object.spawn();}",
    ] {
        assert!(
            Analyzer::analyze(source, false)
                .expect("ordinary method")
                .is_empty()
        );
    }
    assert!(
        Analyzer::analyze(
            "const X:&str=include_str!(\"fixture\"); fn x(){std::fs::read(\"fixture\");}",
            true
        )
        .expect("fixture")
        .is_empty()
    );
    assert!(
        Analyzer::analyze(
            "#[cfg(test)] mod tests { fn x(){std::net::TcpStream::connect(\"x\");} }",
            false
        )
        .expect("cfg test")
        .is_empty()
    );
}

#[test]
fn resolved_normal_dependency_tree_is_confined() {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = std::process::Command::new(cargo)
        .current_dir(&workspace)
        .args([
            "tree",
            "-p",
            "lotta-testkit",
            "--edges",
            "normal",
            "--offline",
            "--prefix",
            "none",
        ])
        .output()
        .expect("run cargo tree");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let tree = String::from_utf8(output.stdout).expect("UTF-8 tree");
    for family in [
        "reqwest",
        "hyper",
        "rand ",
        "getrandom",
        "tempfile",
        "lotta-store",
        "lotta-memfs",
        "lotta-providers",
        "lotta-tools",
        "lotta-channels",
    ] {
        assert!(
            !tree
                .lines()
                .any(|line| line.split_whitespace().next() == Some(family.trim())),
            "forbidden normal dependency {family}:\n{tree}"
        );
    }
}

#[test]
fn dependency_audit_manifests_and_features() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let workspace_manifest = parse_manifest(&workspace.join("Cargo.toml"));
    let testkit_manifest = parse_manifest(&workspace.join("crates/lotta-testkit/Cargo.toml"));
    let workspace_dependencies = table_at(&workspace_manifest, &["workspace", "dependencies"]);
    let testkit_dependencies = table_at(&testkit_manifest, &["dependencies"]);
    for dependency in [
        "reqwest",
        "hyper",
        "rand",
        "getrandom",
        "tempfile",
        "lotta-store",
        "lotta-memfs",
        "lotta-providers",
        "lotta-tools",
        "lotta-channels",
    ] {
        assert!(
            !testkit_dependencies.contains_key(dependency),
            "forbidden testkit dependency: {dependency}"
        );
    }
    let features = workspace_dependencies
        .get("uuid")
        .expect("workspace uuid")
        .get("features")
        .and_then(toml::Value::as_array)
        .expect("uuid features");
    assert!(
        !features
            .iter()
            .any(|feature| feature.as_str() == Some("v4"))
    );
}

fn parse_manifest(path: &Path) -> toml::Value {
    toml::from_str(&std::fs::read_to_string(path).expect("read manifest")).expect("parse manifest")
}
fn table_at<'a>(value: &'a toml::Value, path: &[&str]) -> &'a toml::map::Map<String, toml::Value> {
    let mut current = value;
    for component in path {
        current = current.get(*component).expect("manifest table");
    }
    current.as_table().expect("manifest table value")
}
