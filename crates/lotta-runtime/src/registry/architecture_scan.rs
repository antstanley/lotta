use std::path::{Path, PathBuf};
use syn::visit::{self, Visit};
use syn::{
    Attribute, ExprCall, ExprMacro, GenericArgument, ItemFn, ItemImpl, ItemMacro, ItemMod,
    ItemStatic, Meta, PathArguments, Type,
};

struct Scanner {
    file: PathBuf,
    functions: Vec<String>,
    errors: Vec<String>,
    spawn_count: usize,
}

impl Scanner {
    fn function(&self) -> Option<&str> {
        self.functions.last().map(String::as_str)
    }

    fn reject(&mut self, issue: &str) {
        self.errors.push(format!("{}:{issue}", self.file.display()));
    }
}

impl<'ast> Visit<'ast> for Scanner {
    fn visit_item_fn(&mut self, item: &'ast ItemFn) {
        if test_only(&item.attrs) {
            return;
        }
        self.functions.push(item.sig.ident.to_string());
        visit::visit_item_fn(self, item);
        let _name = self.functions.pop();
    }

    fn visit_item_impl(&mut self, item: &'ast ItemImpl) {
        if !test_only(&item.attrs) {
            visit::visit_item_impl(self, item);
        }
    }

    fn visit_item_mod(&mut self, item: &'ast ItemMod) {
        if !test_only(&item.attrs) {
            visit::visit_item_mod(self, item);
        }
    }

    fn visit_item_static(&mut self, item: &'ast ItemStatic) {
        if test_only(&item.attrs) {
            return;
        }
        if matches!(item.mutability, syn::StaticMutability::Mut(_)) || forbidden_type(&item.ty) {
            self.reject("process_static");
        }
        visit::visit_item_static(self, item);
    }

    fn visit_expr_macro(&mut self, expression: &'ast ExprMacro) {
        if path_string(&expression.mac.path) == "thread_local" {
            self.reject("thread_local");
        }
        visit::visit_expr_macro(self, expression);
    }

    fn visit_item_macro(&mut self, item: &'ast ItemMacro) {
        if path_string(&item.mac.path) == "thread_local" {
            self.reject("thread_local");
        }
        visit::visit_item_macro(self, item);
    }

    fn visit_expr_call(&mut self, call: &'ast ExprCall) {
        if let syn::Expr::Path(path) = call.func.as_ref() {
            let called = path_string(&path.path);
            let allowed = matches!(
                (
                    self.file.file_name().and_then(|name| name.to_str()),
                    self.function()
                ),
                (Some("scope_context.rs"), Some("spawn_scoped"))
                    | (Some("worktree_watcher.rs"), Some("spawn_idle_stop"))
            );
            if is_spawn(&called) {
                self.spawn_count += 1;
                if !allowed {
                    self.reject("unaudited_spawn");
                }
            }
        }
        visit::visit_expr_call(self, call);
    }
}

fn forbidden_type(ty: &Type) -> bool {
    const FORBIDDEN: [&str; 7] = [
        "OnceLock",
        "LazyLock",
        "Mutex",
        "RwLock",
        "Cell",
        "RefCell",
        "UnsafeCell",
    ];
    match ty {
        Type::Array(value) => forbidden_type(&value.elem),
        Type::Group(value) => forbidden_type(&value.elem),
        Type::Paren(value) => forbidden_type(&value.elem),
        Type::Path(path) => path.path.segments.iter().any(|segment| {
            FORBIDDEN.contains(&segment.ident.to_string().as_str())
                || match &segment.arguments {
                    PathArguments::AngleBracketed(arguments) => arguments.args.iter().any(
                        |arg| matches!(arg, GenericArgument::Type(inner) if forbidden_type(inner)),
                    ),
                    _ => false,
                }
        }),
        Type::Ptr(value) => forbidden_type(&value.elem),
        Type::Reference(value) => forbidden_type(&value.elem),
        Type::Slice(value) => forbidden_type(&value.elem),
        Type::Tuple(value) => value.elems.iter().any(forbidden_type),
        _ => false,
    }
}

fn test_only(attributes: &[Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        let Meta::List(list) = &attribute.meta else {
            return false;
        };
        attribute.path().is_ident("cfg") && list.tokens.to_string().contains("test")
    })
}

fn path_string(path: &syn::Path) -> String {
    path.segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}

fn is_spawn(path: &str) -> bool {
    matches!(
        path,
        "tokio::spawn" | "tokio::task::spawn" | "std::thread::spawn"
    )
}

fn scan_source(path: &Path, source: &str) -> Result<usize, Vec<String>> {
    let syntax = syn::parse_file(source).map_err(|error| vec![error.to_string()])?;
    let mut scanner = Scanner {
        file: path.to_owned(),
        functions: Vec::new(),
        errors: Vec::new(),
        spawn_count: 0,
    };
    scanner.visit_file(&syntax);
    if scanner.errors.is_empty() {
        Ok(scanner.spawn_count)
    } else {
        Err(scanner.errors)
    }
}

fn production_files() -> Vec<PathBuf> {
    let source_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    walkdir::WalkDir::new(&source_root)
        .into_iter()
        .filter_map(Result::ok)
        .map(walkdir::DirEntry::into_path)
        .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
        .filter(|path| {
            !path
                .components()
                .any(|component| component.as_os_str() == "tests")
        })
        .filter(|path| path.file_name().is_none_or(|name| name != "tests.rs"))
        .filter(|path| {
            let relative = path.strip_prefix(&source_root).unwrap_or(path);
            !matches!(
                relative.to_str(),
                Some(
                    "registry/keying.rs"
                        | "registry/eviction.rs"
                        | "registry/ambient_is_task_local.rs"
                        | "registry/architecture_scan.rs"
                        | "worktree_watcher/idle_stop.rs"
                )
            )
        })
        .collect()
}

#[test]
fn every_production_file_passes_structural_scan() {
    let files = production_files();
    assert!(files.len() >= 4);
    let mut spawns = 0;
    for path in files {
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        spawns += scan_source(&path, &source).unwrap_or_else(|errors| panic!("{errors:?}"));
    }
    assert_eq!(spawns, 2);
}

#[test]
fn mutation_cases_are_rejected() {
    let cases = [
        "fn bad(){tokio::spawn(async {});}",
        "fn spawn_scoped(){} fn bad(){tokio::spawn(async {});}",
        "fn bad(){tokio::task::spawn(async {});}",
        "fn bad(){std::thread::spawn(|| {});}",
        "static mut BAD: usize = 0;",
        concat!(
            "static BAD: std::sync::OnceLock<std::sync::Mutex<ListenerRuntime>> = ",
            "std::sync::OnceLock::new();",
        ),
        "thread_local! { static BAD: usize = 0; }",
    ];
    for source in cases {
        assert!(
            scan_source(Path::new("fourth.rs"), source).is_err(),
            "accepted {source}"
        );
    }
    let valid = include_str!("../scope_context.rs");
    let appended = format!("{valid}\nfn bad() {{ tokio::spawn(async {{}}); }}");
    assert!(scan_source(Path::new("scope_context.rs"), &appended).is_err());
    let test_spawn = "#[cfg(test)] fn allowed_test() { tokio::spawn(async {}); }";
    assert_eq!(scan_source(Path::new("fourth.rs"), test_spawn), Ok(0));
}
