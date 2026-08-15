//! Source hygiene checks for production protocol sources.

use proc_macro2::Span;
use std::fs;
use std::path::{Path, PathBuf};
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{Attribute, Item, Meta};

const FILE_LINES_MAX: usize = 1_000;
const LINE_BYTES_MAX: usize = 100;
const FUNCTION_LINES_MAX: usize = 70;

fn sources() -> Vec<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut paths = Vec::new();
    collect_rs(&root.join("src"), &mut paths);
    paths.push(root.join("../../tools/extract-protocol-fixture.mjs"));
    paths
}

fn collect_rs(directory: &Path, paths: &mut Vec<PathBuf>) {
    let mut entries = fs::read_dir(directory)
        .expect("read source directory")
        .map(|entry| entry.expect("read directory entry").path())
        .collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_rs(&path, paths);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            paths.push(path);
        }
    }
}

fn inspect_source(path: &Path, text: &str) -> Result<(), String> {
    inspect_hard_limits(path, text)?;
    if path.extension().is_some_and(|ext| ext == "rs") {
        inspect_rust_source(path, text)?;
    }
    Ok(())
}

fn inspect_hard_limits(path: &Path, text: &str) -> Result<(), String> {
    let lines = text.lines().collect::<Vec<_>>();
    if lines.len() > FILE_LINES_MAX {
        return Err(format!("{} has {} lines", path.display(), lines.len()));
    }
    for (index, line) in lines.iter().enumerate() {
        if line.len() > LINE_BYTES_MAX {
            return Err(format!(
                "{}:{} has {} UTF-8 bytes",
                path.display(),
                index + 1,
                line.len()
            ));
        }
    }
    Ok(())
}

fn inspect_rust_source(path: &Path, text: &str) -> Result<(), String> {
    let file = syn::parse_file(text)
        .map_err(|error| format!("{} does not parse: {error}", path.display()))?;
    let mut visitor = HygieneVisitor { path, error: None };
    visitor.visit_file(&file);
    visitor.error.map_or(Ok(()), Err)
}

struct HygieneVisitor<'a> {
    path: &'a Path,
    error: Option<String>,
}

impl HygieneVisitor<'_> {
    fn inspect_attrs(&mut self, attrs: &[Attribute]) -> bool {
        if test_only(attrs) {
            return false;
        }
        for attr in attrs {
            if production_allow(&attr.meta) {
                self.fail(attr.span(), "contains a production allow suppression");
            }
        }
        self.error.is_none()
    }

    fn inspect_function(&mut self, attrs: &[Attribute], span: Span) -> bool {
        if !self.inspect_attrs(attrs) {
            return false;
        }
        let start = span.start().line;
        let end = span.end().line;
        let length = end - start + 1;
        if length > FUNCTION_LINES_MAX {
            self.fail(span, &format!("function has {length} lines"));
        }
        self.error.is_none()
    }

    fn fail(&mut self, span: Span, message: &str) {
        if self.error.is_none() {
            self.error = Some(format!(
                "{}:{} {message}",
                self.path.display(),
                span.start().line
            ));
        }
    }
}

impl<'ast> Visit<'ast> for HygieneVisitor<'_> {
    fn visit_file(&mut self, node: &'ast syn::File) {
        if self.inspect_attrs(&node.attrs) {
            visit::visit_file(self, node);
        }
    }

    fn visit_item(&mut self, node: &'ast Item) {
        if self.error.is_some() || !self.inspect_attrs(item_attrs(node)) {
            return;
        }
        visit::visit_item(self, node);
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if self.inspect_function(&node.attrs, node.span()) {
            visit::visit_item_fn(self, node);
        }
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        if self.inspect_function(&node.attrs, node.span()) {
            visit::visit_impl_item_fn(self, node);
        }
    }

    fn visit_trait_item_fn(&mut self, node: &'ast syn::TraitItemFn) {
        if self.inspect_function(&node.attrs, node.span()) {
            visit::visit_trait_item_fn(self, node);
        }
    }

    fn visit_foreign_item_fn(&mut self, node: &'ast syn::ForeignItemFn) {
        let _ = self.inspect_function(&node.attrs, node.span());
    }

    fn visit_attribute(&mut self, node: &'ast Attribute) {
        if self.error.is_none() && production_allow(&node.meta) {
            self.fail(node.span(), "contains a production allow suppression");
        }
    }
}

fn item_attrs(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(item) => &item.attrs,
        Item::Enum(item) => &item.attrs,
        Item::ExternCrate(item) => &item.attrs,
        Item::Fn(item) => &item.attrs,
        Item::ForeignMod(item) => &item.attrs,
        Item::Impl(item) => &item.attrs,
        Item::Macro(item) => &item.attrs,
        Item::Mod(item) => &item.attrs,
        Item::Static(item) => &item.attrs,
        Item::Struct(item) => &item.attrs,
        Item::Trait(item) => &item.attrs,
        Item::TraitAlias(item) => &item.attrs,
        Item::Type(item) => &item.attrs,
        Item::Union(item) => &item.attrs,
        Item::Use(item) => &item.attrs,
        Item::Verbatim(_) | _ => &[],
    }
}

fn test_only(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("test") || (attr.path().is_ident("cfg") && meta_test_only(&attr.meta))
    })
}

fn meta_test_only(meta: &Meta) -> bool {
    if meta.path().is_ident("cfg") {
        let Meta::List(list) = meta else {
            return false;
        };
        return list
            .parse_args::<Meta>()
            .is_ok_and(|meta| cfg_test_only(&meta));
    }
    cfg_test_only(meta)
}

fn cfg_test_only(meta: &Meta) -> bool {
    if meta.path().is_ident("test") {
        return true;
    }
    let Meta::List(list) = meta else {
        return false;
    };
    let Ok(metas) =
        list.parse_args_with(syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated)
    else {
        return false;
    };
    if list.path.is_ident("all") {
        metas.iter().any(cfg_test_only)
    } else if list.path.is_ident("any") {
        !metas.is_empty() && metas.iter().all(cfg_test_only)
    } else {
        false
    }
}

fn production_allow(meta: &Meta) -> bool {
    if meta.path().is_ident("allow") {
        return true;
    }
    if !meta.path().is_ident("cfg_attr") {
        return false;
    }
    let Meta::List(list) = meta else {
        return false;
    };
    let Ok(args) =
        list.parse_args_with(syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated)
    else {
        return false;
    };
    let mut args = args.iter();
    let Some(condition) = args.next() else {
        return false;
    };
    !cfg_test_only(condition) && args.any(production_allow)
}

#[test]
fn source_hygiene_limits_production_sources() {
    for path in sources() {
        let text = fs::read_to_string(&path).expect("read source file");
        inspect_source(&path, &text).unwrap_or_else(|error| panic!("{error}"));
    }
}

fn lines(count: usize) -> String {
    "let _value = 1;\n".repeat(count)
}

fn function(prefix: &str, body_lines: usize) -> String {
    format!("{prefix} {{\n{}}}", lines(body_lines))
}

fn rejected(source: &str) {
    assert!(
        inspect_source(Path::new("synthetic.rs"), source).is_err(),
        "mutation unexpectedly passed: {source}"
    );
}

fn accepted(source: &str) {
    inspect_source(Path::new("synthetic.rs"), source).expect("source should pass");
}

#[test]
fn source_hygiene_mutation_matrix() {
    rejected(&"x".repeat(LINE_BYTES_MAX + 1));
    rejected(&"x\n".repeat(FILE_LINES_MAX + 1));
    rejected(&function("fn plain(\n)", 68));
    rejected(&function(
        "pub async fn generic<T>(\n_value: T,\n) where T: Copy",
        67,
    ));
    rejected(&format!(
        "struct S;\nimpl S {{\n{}\n}}",
        function("pub const unsafe fn method(\n)", 68)
    ));
    rejected(&format!(
        "trait T {{\n{}\n}}",
        function("fn default(\n)", 68)
    ));
    rejected(&format!(
        "extern \"C\" {{\n    fn foreign(\n{}    );\n}}",
        "        value: i32,\n".repeat(69)
    ));
    accepted(&format!("#[cfg(test)]\n{}", function("fn test_only()", 70)));
    accepted(&format!(
        "#[cfg(any(test, all(test, feature = \"x\")))]\nmod checks {{\n{}\n}}",
        function("fn test_only()", 70)
    ));
    rejected(&format!(
        "#[cfg(test)]\nmod tests {{}}\n{}",
        function("fn production()", 70)
    ));
    let confusing = concat!(
        "fn data() {\n",
        "    let _ = r###\"} fn fake() { #[allow(dead_code)] }\"###;\n",
        "    // } fn no() {\n",
        "}\n"
    );
    accepted(confusing);
    rejected("#[allow(dead_code)]\nfn hidden() {}");
    rejected("#![allow(dead_code)]\nfn hidden() {}");
    rejected("#[cfg_attr(unix, allow(dead_code))]\nfn hidden() {}");
    rejected("#[cfg_attr(unix, cfg_attr(windows, allow(dead_code)))]\nfn hidden() {}");
    accepted("#[cfg(test)]\n#[allow(dead_code)]\nfn hidden() {}");
    accepted("#[cfg_attr(test, allow(dead_code))]\nfn visible() {}");
    accepted(&function("fn boundary()", 68));
}
