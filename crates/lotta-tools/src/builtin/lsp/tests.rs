use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct FakeServer {
    values: Vec<Diagnostic>,
    calls: AtomicUsize,
}
impl LanguageServerPort for FakeServer {
    fn diagnostics(
        &self,
        _: &'_ Path,
        _: tokio_util::sync::CancellationToken,
        _: std::time::Duration,
    ) -> DiagnosticsFuture<'_> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let values = self.values.clone();
        Box::pin(async move { Ok(values) })
    }
}

#[test]
fn diagnostics_execution_is_exact_and_bounded() {
    let values = vec![
        Diagnostic {
            severity: DiagnosticSeverity::Error,
            line: 0,
            character: 1,
            message: "broken".into(),
        },
        Diagnostic {
            severity: DiagnosticSeverity::Warning,
            line: 2,
            character: 3,
            message: "warning".into(),
        },
    ];
    assert_eq!(
        format_diagnostics(validate_diagnostics(values).unwrap()).unwrap(),
        "ERROR [1:2] broken\n"
    );
    let at = vec![
        Diagnostic {
            severity: DiagnosticSeverity::Error,
            line: 0,
            character: 0,
            message: "x".into()
        };
        LSP_DIAGNOSTICS_ITEMS_MAX
    ];
    assert!(validate_diagnostics(at).is_ok());
    let above = vec![
        Diagnostic {
            severity: DiagnosticSeverity::Error,
            line: 0,
            character: 0,
            message: "x".into()
        };
        LSP_DIAGNOSTICS_ITEMS_MAX + 1
    ];
    assert_eq!(validate_diagnostics(above), Err(LanguageServerError::Limit));
}

#[tokio::test]
async fn configured_language_server_is_invoked() {
    let root = temp_root();
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let file = root.join("sample.rs");
    std::fs::write(&file, "fn main() {}").unwrap();
    let server = Arc::new(FakeServer {
        values: vec![Diagnostic {
            severity: DiagnosticSeverity::Error,
            line: 0,
            character: 0,
            message: "broken".into(),
        }],
        calls: AtomicUsize::new(0),
    });
    let trait_server: Arc<dyn LanguageServerPort> = server.clone();
    let registry =
        LanguageServerRegistry::new(root.clone(), [("rs".into(), trait_server)]).unwrap();
    let (path, selected) = registry.resolve(file.to_str().unwrap()).unwrap();
    let values = selected
        .diagnostics(
            &path,
            tokio_util::sync::CancellationToken::new(),
            std::time::Duration::from_secs(1),
        )
        .await
        .unwrap();
    assert_eq!(values.len(), 1);
    assert_eq!(server.calls.load(Ordering::Relaxed), 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn unconfigured_language_is_fixed_error() {
    let root = temp_root();
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let file = root.join("sample.unknown");
    std::fs::write(&file, "text").unwrap();
    let registry = LanguageServerRegistry::new(root.clone(), []).unwrap();
    assert!(matches!(
        registry.resolve(file.to_str().unwrap()),
        Err(LanguageServerError::Unconfigured)
    ));
    std::fs::remove_dir_all(root).unwrap();
}

fn temp_root() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    std::env::temp_dir().join(format!(
        "lotta_lsp_{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}
