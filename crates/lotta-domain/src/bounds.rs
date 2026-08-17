/// Action required when a resource bound is reached.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoundReached {
    /// Reject new work observably.
    Reject,
    /// Fail the owning invariant observably.
    FailInvariant,
    /// Replace only eligible pending work.
    CoalescePendingWork,
    /// Rotate retained log data.
    RotateLog,
}

/// Immutable metadata describing an observable resource bound.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceBound {
    /// Stable bound name.
    pub name: &'static str,
    /// Default maximum or retention value.
    pub value: usize,
    /// Structured event emitted at the bound.
    pub event: &'static str,
    /// Counter incremented at the bound.
    pub counter: &'static str,
    /// Required at-limit action.
    pub reached: BoundReached,
}

/// Structured observation emitted when a resource reaches or exceeds its bound.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BoundObservation {
    /// Stable bound name.
    pub name: &'static str,
    /// Structured event emitted at the bound.
    pub event: &'static str,
    /// Counter incremented at the bound.
    pub counter: &'static str,
    /// Required at-limit action.
    pub reached: BoundReached,
    /// Actual observed resource amount.
    pub actual: usize,
    /// Whether the resource exceeded, rather than exactly reached, the bound.
    pub exceeded: bool,
}

impl ResourceBound {
    /// Returns a structured observation when `actual` reaches or exceeds this bound.
    #[must_use]
    pub const fn observe(self, actual: usize) -> Option<BoundObservation> {
        if actual < self.value {
            return None;
        }
        Some(BoundObservation {
            name: self.name,
            event: self.event,
            counter: self.counter,
            reached: self.reached,
            actual,
            exceeded: actual > self.value,
        })
    }
}

macro_rules! bound {
    ($name:ident, $value:expr, $event:literal, $counter:literal, $reached:ident) => {
        #[doc = concat!("Resource bound `", stringify!($name), "`.")]
        pub const $name: ResourceBound = ResourceBound {
            name: stringify!($name),
            value: $value,
            event: $event,
            counter: $counter,
            reached: BoundReached::$reached,
        };
    };
}

bound!(
    CONNECTIONS_MAX,
    1_024,
    "connection_limit",
    "connections_limit_total",
    Reject
);
bound!(
    RUNTIMES_MAX,
    4_096,
    "runtime_limit",
    "runtimes_limit_total",
    Reject
);
bound!(
    RUNTIME_SUBSCRIPTIONS_PER_CONNECTION_MAX,
    256,
    "subscription_limit",
    "subscription_limit_total",
    Reject
);
bound!(
    QUEUE_ITEMS_SOFT_MAX,
    100,
    "queue_soft_limit",
    "queue_soft_limit_total",
    CoalescePendingWork
);
bound!(
    QUEUE_ITEMS_HARD_MAX,
    300,
    "buffer_limit",
    "queue_hard_limit_total",
    Reject
);
bound!(
    PENDING_APPROVALS_PER_RUNTIME_MAX,
    128,
    "approval_limit",
    "approval_limit_total",
    FailInvariant
);
bound!(
    EXTERNAL_TOOLS_PER_RUNTIME_MAX,
    256,
    "external_tool_limit",
    "external_tool_limit_total",
    Reject
);
bound!(
    MCP_SERVERS_PER_AGENT_MAX,
    64,
    "mcp_server_limit",
    "mcp_server_limit_total",
    Reject
);
bound!(
    MCP_TOOLS_PER_SERVER_MAX,
    512,
    "mcp_tool_limit",
    "mcp_tool_limit_total",
    Reject
);
bound!(
    SCHEDULE_RUN_LOG_KEEP_LINES,
    2_000,
    "schedule_log_line_limit",
    "schedule_log_rotation_total",
    RotateLog
);
bound!(
    SCHEDULE_RUN_LOG_BYTES_MAX,
    2_000_000,
    "schedule_log_byte_limit",
    "schedule_log_rotation_total",
    RotateLog
);

pub(crate) const EXTRAS_FIELDS_MAX: usize = 128;
pub(crate) const STRING_ITEMS_MAX: usize = 1_024;
pub(crate) const UNBOUNDED_COLLECTION_ITEMS_MAX: usize = 4_096;
/// Canonical maximum fields retained by open compatible entity maps.
pub const UNBOUNDED_MAP_FIELDS_MAX: usize = 1_024;
pub(crate) const JSON_DEPTH_MAX: usize = 64;
pub(crate) const JSON_ITEMS_MAX: usize = 4_096;
pub(crate) const JSON_PROPERTIES_MAX: usize = 1_024;
pub(crate) const PERMISSION_SUGGESTIONS_ITEMS_MAX: usize = 128;
pub(crate) const DIFFS_ITEMS_MAX: usize = 1_024;
pub(crate) const ADMISSION_HISTORY_ITEMS_MAX: usize = 300;
/// Maximum queue items consumed before the caller must yield.
pub const QUEUE_PUMP_BATCH_MAX: usize = 64;

/// The eleven canonical resource bounds in specification order.
pub const RESOURCE_BOUNDS: [ResourceBound; 11] = [
    CONNECTIONS_MAX,
    RUNTIMES_MAX,
    RUNTIME_SUBSCRIPTIONS_PER_CONNECTION_MAX,
    QUEUE_ITEMS_SOFT_MAX,
    QUEUE_ITEMS_HARD_MAX,
    PENDING_APPROVALS_PER_RUNTIME_MAX,
    EXTERNAL_TOOLS_PER_RUNTIME_MAX,
    MCP_SERVERS_PER_AGENT_MAX,
    MCP_TOOLS_PER_SERVER_MAX,
    SCHEDULE_RUN_LOG_KEEP_LINES,
    SCHEDULE_RUN_LOG_BYTES_MAX,
];

#[cfg(test)]
mod scanner_tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use syn::visit::{self, Visit};
    use syn::{Attribute, BinOp, Expr, ExprBinary, ExprCall, ExprLit, ExprMethodCall};
    use syn::{File, Item, ItemConst, ItemStatic, Lit, Meta, Path as SynPath};

    #[derive(Debug, Eq, PartialEq)]
    struct LintFinding {
        line: usize,
        rule: &'static str,
        text: String,
    }

    fn rust_sources(root: &Path) -> Vec<PathBuf> {
        let mut pending = vec![root.to_owned()];
        let mut sources = Vec::new();
        while let Some(path) = pending.pop() {
            for entry in fs::read_dir(path).expect("domain source directory is readable") {
                let path = entry.expect("domain source entry is readable").path();
                if path.is_dir() {
                    pending.push(path);
                } else if path.extension().is_some_and(|extension| extension == "rs")
                    && path.file_name().is_some_and(|name| name != "bounds.rs")
                    && !is_test_only_file(&path)
                {
                    sources.push(path);
                }
            }
        }
        sources.sort();
        sources
    }

    fn is_test_only_file(path: &Path) -> bool {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == "tests.rs" || name.ends_with("_tests.rs"))
    }

    fn scan_source(source: &str) -> Vec<LintFinding> {
        let file = syn::parse_file(source).expect("fixture or repository source must parse");
        scan_file(&file)
    }

    fn scan_file(file: &File) -> Vec<LintFinding> {
        let mut scanner = BoundScanner::default();
        scanner.visit_file(file);
        scanner.findings
    }

    #[derive(Default)]
    struct BoundScanner {
        findings: Vec<LintFinding>,
    }

    impl BoundScanner {
        fn report(&mut self, rule: &'static str, text: impl Into<String>) {
            self.findings.push(LintFinding {
                line: 0,
                rule,
                text: text.into(),
            });
        }
    }

    impl<'ast> Visit<'ast> for BoundScanner {
        fn visit_item(&mut self, item: &'ast Item) {
            if !item_attributes(item).is_some_and(|attributes| attributes.iter().any(cfg_test)) {
                visit::visit_item(self, item);
            }
        }

        fn visit_item_const(&mut self, item: &'ast ItemConst) {
            if bound_like_name(&item.ident.to_string()) && contains_resource_literal(&item.expr) {
                self.report("bound-like declaration", item.ident.to_string());
            }
            visit::visit_item_const(self, item);
        }

        fn visit_item_static(&mut self, item: &'ast ItemStatic) {
            if bound_like_name(&item.ident.to_string()) && contains_resource_literal(&item.expr) {
                self.report("bound-like declaration", item.ident.to_string());
            }
            visit::visit_item_static(self, item);
        }

        fn visit_expr_binary(&mut self, expression: &'ast ExprBinary) {
            if comparison(&expression.op)
                && ((is_len_or_capacity(&expression.left)
                    && contains_resource_literal(&expression.right))
                    || (is_len_or_capacity(&expression.right)
                        && contains_resource_literal(&expression.left)))
            {
                self.report("raw numeric limit operation", "length/capacity comparison");
            }
            visit::visit_expr_binary(self, expression);
        }

        fn visit_expr_call(&mut self, expression: &'ast ExprCall) {
            if call_name(&expression.func)
                .as_deref()
                .is_some_and(limit_operation)
                && expression.args.iter().any(contains_resource_literal)
            {
                self.report("raw numeric limit operation", "qualified limit call");
            }
            visit::visit_expr_call(self, expression);
        }

        fn visit_expr_method_call(&mut self, expression: &'ast ExprMethodCall) {
            if limit_operation(&expression.method.to_string())
                && expression.args.iter().any(contains_resource_literal)
            {
                self.report("raw numeric limit operation", "limit method call");
            }
            visit::visit_expr_method_call(self, expression);
        }

        fn visit_macro(&mut self, expression: &'ast syn::Macro) {
            if macro_name(&expression.path)
                .as_deref()
                .is_some_and(assertion_macro)
            {
                let tokens = expression.tokens.to_string();
                if assertion_has_raw_limit(&tokens) {
                    self.report("raw numeric limit operation", "assertion limit");
                }
                return;
            }
            visit::visit_macro(self, expression);
        }
    }

    fn item_attributes(item: &Item) -> Option<&[Attribute]> {
        match item {
            Item::Const(item) => Some(&item.attrs),
            Item::Enum(item) => Some(&item.attrs),
            Item::Fn(item) => Some(&item.attrs),
            Item::Impl(item) => Some(&item.attrs),
            Item::Macro(item) => Some(&item.attrs),
            Item::Mod(item) => Some(&item.attrs),
            Item::Static(item) => Some(&item.attrs),
            Item::Struct(item) => Some(&item.attrs),
            Item::Trait(item) => Some(&item.attrs),
            Item::Type(item) => Some(&item.attrs),
            Item::Union(item) => Some(&item.attrs),
            Item::Use(item) => Some(&item.attrs),
            _ => None,
        }
    }

    fn cfg_test(attribute: &Attribute) -> bool {
        attribute.path().is_ident("cfg")
            && matches!(&attribute.meta, Meta::List(list) if list.tokens.to_string() == "test")
    }

    fn bound_like_name(name: &str) -> bool {
        let name = name.to_ascii_uppercase();
        name.ends_with("_MAX")
            || name.ends_with("_LIMIT")
            || [
                "KEEP",
                "BYTES",
                "LINES",
                "ITEMS",
                "FIELDS",
                "DEPTH",
                "PROPERTIES",
                "CAPACITY",
                "RETENTION",
                "THRESHOLD",
            ]
            .into_iter()
            .any(|unit| name.split('_').any(|part| part == unit))
    }

    fn contains_resource_literal(expression: &Expr) -> bool {
        struct LiteralFinder(bool);
        impl<'ast> Visit<'ast> for LiteralFinder {
            fn visit_expr_lit(&mut self, expression: &'ast ExprLit) {
                let is_resource = matches!(
                    &expression.lit,
                    Lit::Int(value)
                        if value.base10_parse::<u128>().is_ok_and(|value| value > 1)
                );
                if is_resource {
                    self.0 = true;
                }
            }
        }
        let mut finder = LiteralFinder(false);
        finder.visit_expr(expression);
        finder.0
    }

    fn comparison(operator: &BinOp) -> bool {
        matches!(
            operator,
            BinOp::Eq(_) | BinOp::Ne(_) | BinOp::Lt(_) | BinOp::Le(_) | BinOp::Gt(_) | BinOp::Ge(_)
        )
    }

    fn is_len_or_capacity(expression: &Expr) -> bool {
        match expression {
            Expr::MethodCall(call) => {
                matches!(call.method.to_string().as_str(), "len" | "capacity")
                    && call.args.is_empty()
            }
            Expr::Paren(expression) => is_len_or_capacity(&expression.expr),
            _ => false,
        }
    }

    fn call_name(expression: &Expr) -> Option<String> {
        match expression {
            Expr::Path(path) => path
                .path
                .segments
                .last()
                .map(|segment| segment.ident.to_string()),
            _ => None,
        }
    }

    fn macro_name(path: &SynPath) -> Option<String> {
        path.segments
            .last()
            .map(|segment| segment.ident.to_string())
    }

    fn limit_operation(name: &str) -> bool {
        matches!(
            name,
            "with_capacity" | "reserve" | "reserve_exact" | "truncate" | "take" | "limit"
        )
    }

    fn assertion_macro(name: &str) -> bool {
        matches!(
            name,
            "assert" | "assert_eq" | "assert_ne" | "debug_assert" | "debug_assert_eq"
        )
    }

    fn assertion_has_raw_limit(tokens: &str) -> bool {
        let compact: String = tokens
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect();
        let has_subject = compact.contains(".len()") || compact.contains(".capacity()");
        let has_literal = compact
            .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .filter_map(|token| token.parse::<u128>().ok())
            .any(|value| value > 1);
        has_subject && has_literal
    }

    #[test]
    fn no_bound_literals_outside_bounds_module() {
        let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let sources = rust_sources(&source_root);
        let findings: Vec<_> = sources
            .iter()
            .flat_map(|path| {
                let source = fs::read_to_string(path).expect("Rust source is readable");
                let relative = path
                    .strip_prefix(&source_root)
                    .expect("source is below root")
                    .to_owned();
                scan_source(&source).into_iter().map(move |finding| {
                    format!(
                        "{}:{}: {}: {}",
                        relative.display(),
                        finding.line,
                        finding.rule,
                        finding.text
                    )
                })
            })
            .collect();
        assert!(
            findings.is_empty(),
            "resource bounds belong in bounds.rs:\n{}",
            findings.join("\n")
        );
    }

    fn assert_rules(source: &str, expected: &[&str]) {
        let actual: Vec<_> = scan_source(source)
            .into_iter()
            .map(|finding| finding.rule)
            .collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn scanner_rejects_multiline_qualified_declarations() {
        assert_rules(
            r"
#[allow(dead_code)]
pub(crate) const
    PAYLOAD_BYTES_LIMIT:
    core::primitive::usize =
    crate::helper::base() + 8_192;
pub static CACHE_ITEMS_MAX: core::primitive::usize = 500;
",
            &["bound-like declaration", "bound-like declaration"],
        );
    }

    #[test]
    fn scanner_rejects_multiline_and_qualified_limit_calls() {
        assert_rules(
            r"
fn reject(values: &mut Vec<u8>) {
    let _ = std::vec::Vec::<u8>::with_capacity(
        4_096
    );
    values.reserve_exact(
        128
    );
    Iterator::take(values.iter(), 64);
}
",
            &[
                "raw numeric limit operation",
                "raw numeric limit operation",
                "raw numeric limit operation",
            ],
        );
    }

    #[test]
    fn scanner_rejects_reversed_len_and_assertion_bounds() {
        assert_rules(
            r"
fn reject(values: &[u8]) {
    if 128 < values.len() {}
    assert!(values.len() <= 256);
    assert_eq!(values.capacity(), 512);
}
",
            &[
                "raw numeric limit operation",
                "raw numeric limit operation",
                "raw numeric limit operation",
            ],
        );
    }

    #[test]
    fn scanner_excludes_cfg_test_and_continues_after_it() {
        assert_rules(
            r"
#[cfg(test)]
mod tests { const BAD_ITEMS_MAX: usize = 3; }
#[cfg(test)]
fn fixture(values: &[u8]) { assert!(values.len() < 8); }
fn production(values: &[u8]) { values.iter().take(20); }
",
            &["raw numeric limit operation"],
        );
    }

    #[test]
    fn scanner_accepts_benign_numeric_code_and_named_bounds() {
        assert_rules(
            r"
const SEQUENCE_MIN: u64 = 1;
enum Kind { First = 1, Second = 2 }
fn next(generation: u64, id: u128, values: &[u8]) {
    let _ = generation.checked_add(1);
    let _ = id == 42_u128;
    let _ = values.len() > ITEMS_MAX;
    let _ = Vec::<u8>::with_capacity(crate::bounds::CAPACITY_MAX);
}
",
            &[],
        );
    }

    #[test]
    fn source_scope_excludes_only_test_files_and_bounds() {
        assert!(is_test_only_file(Path::new("entities/tests.rs")));
        assert!(is_test_only_file(Path::new("entities/channel_tests.rs")));
        assert!(!is_test_only_file(Path::new(
            "entities/schema_conformance.rs"
        )));
        assert!(!is_test_only_file(Path::new("runtime/mod.rs")));
    }
}

#[cfg(test)]
pub(crate) fn assert_matches_spec_table() {
    let rows = [
        CONNECTIONS_MAX,
        RUNTIMES_MAX,
        RUNTIME_SUBSCRIPTIONS_PER_CONNECTION_MAX,
        QUEUE_ITEMS_SOFT_MAX,
        QUEUE_ITEMS_HARD_MAX,
        PENDING_APPROVALS_PER_RUNTIME_MAX,
        EXTERNAL_TOOLS_PER_RUNTIME_MAX,
        MCP_SERVERS_PER_AGENT_MAX,
        MCP_TOOLS_PER_SERVER_MAX,
        SCHEDULE_RUN_LOG_KEEP_LINES,
        SCHEDULE_RUN_LOG_BYTES_MAX,
    ];
    assert_eq!(RESOURCE_BOUNDS, rows);
}

#[cfg(test)]
pub(crate) fn assert_units_last_naming() {
    for bound in RESOURCE_BOUNDS {
        assert!(!bound.name.starts_with("MAX_"));
        assert!(bound.name.ends_with("_MAX") || bound.name.ends_with("_LINES"));
    }
}

#[cfg(test)]
type ExpectedBound = (
    &'static str,
    usize,
    &'static str,
    &'static str,
    BoundReached,
);

#[cfg(test)]
fn expected_core_bounds() -> [ExpectedBound; 6] {
    [
        (
            "CONNECTIONS_MAX",
            1_024,
            "connection_limit",
            "connections_limit_total",
            BoundReached::Reject,
        ),
        (
            "RUNTIMES_MAX",
            4_096,
            "runtime_limit",
            "runtimes_limit_total",
            BoundReached::Reject,
        ),
        (
            "RUNTIME_SUBSCRIPTIONS_PER_CONNECTION_MAX",
            256,
            "subscription_limit",
            "subscription_limit_total",
            BoundReached::Reject,
        ),
        (
            "QUEUE_ITEMS_SOFT_MAX",
            100,
            "queue_soft_limit",
            "queue_soft_limit_total",
            BoundReached::CoalescePendingWork,
        ),
        (
            "QUEUE_ITEMS_HARD_MAX",
            300,
            "buffer_limit",
            "queue_hard_limit_total",
            BoundReached::Reject,
        ),
        (
            "PENDING_APPROVALS_PER_RUNTIME_MAX",
            128,
            "approval_limit",
            "approval_limit_total",
            BoundReached::FailInvariant,
        ),
    ]
}

#[cfg(test)]
fn expected_extension_bounds() -> [ExpectedBound; 5] {
    [
        (
            "EXTERNAL_TOOLS_PER_RUNTIME_MAX",
            256,
            "external_tool_limit",
            "external_tool_limit_total",
            BoundReached::Reject,
        ),
        (
            "MCP_SERVERS_PER_AGENT_MAX",
            64,
            "mcp_server_limit",
            "mcp_server_limit_total",
            BoundReached::Reject,
        ),
        (
            "MCP_TOOLS_PER_SERVER_MAX",
            512,
            "mcp_tool_limit",
            "mcp_tool_limit_total",
            BoundReached::Reject,
        ),
        (
            "SCHEDULE_RUN_LOG_KEEP_LINES",
            2_000,
            "schedule_log_line_limit",
            "schedule_log_rotation_total",
            BoundReached::RotateLog,
        ),
        (
            "SCHEDULE_RUN_LOG_BYTES_MAX",
            2_000_000,
            "schedule_log_byte_limit",
            "schedule_log_rotation_total",
            BoundReached::RotateLog,
        ),
    ]
}

#[cfg(test)]
fn assert_bound_observations(bound: ResourceBound, row: ExpectedBound) {
    let (name, value, event, counter, reached) = row;
    assert_eq!(
        (
            bound.name,
            bound.value,
            bound.event,
            bound.counter,
            bound.reached
        ),
        row
    );
    assert!(bound.observe(value - 1).is_none());
    assert_eq!(
        bound.observe(value),
        Some(BoundObservation {
            name,
            event,
            counter,
            reached,
            actual: value,
            exceeded: false,
        })
    );
    assert_eq!(
        bound.observe(value + 1),
        Some(BoundObservation {
            name,
            event,
            counter,
            reached,
            actual: value + 1,
            exceeded: true,
        })
    );
}

#[cfg(test)]
fn assert_reached_matrix() {
    let count = |reached| {
        RESOURCE_BOUNDS
            .iter()
            .filter(|b| b.reached == reached)
            .count()
    };
    assert_eq!(count(BoundReached::Reject), 7);
    assert_eq!(count(BoundReached::CoalescePendingWork), 1);
    assert_eq!(count(BoundReached::FailInvariant), 1);
    assert_eq!(count(BoundReached::RotateLog), 2);
}

#[cfg(test)]
pub(crate) fn assert_at_limit_is_observable() {
    let rows = expected_core_bounds()
        .into_iter()
        .chain(expected_extension_bounds());
    for (bound, row) in RESOURCE_BOUNDS.into_iter().zip(rows) {
        assert_bound_observations(bound, row);
    }
    assert_reached_matrix();
}

#[cfg(test)]
#[test]
fn matches_spec_table() {
    assert_matches_spec_table();
}

#[cfg(test)]
#[test]
fn units_last_naming() {
    assert_units_last_naming();
}

#[cfg(test)]
#[test]
fn at_limit_is_observable() {
    assert_at_limit_is_observable();
}
