use super::*;
use crate::GitMemFs;
use crate::tests::{TestRoot, content, empty, message, path, valid};
use lotta_domain::{AgentId, ConversationId, Timestamp};
use lotta_runtime::ports::MemFsPort;
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio_util::sync::CancellationToken;

fn timestamp(value: &str) -> Timestamp {
    Timestamp::parse_persisted_rfc3339(value).expect("timestamp")
}
fn prompt_text(value: &str) -> PromptText {
    PromptText::new(value.to_owned()).expect("prompt text")
}
fn agent() -> AgentId {
    AgentId::accept("agent-local-prompt").expect("agent")
}
fn conversation() -> ConversationId {
    ConversationId::accept("conversation-prompt").expect("conversation")
}
fn inputs(raw: &str, at: &str) -> PromptInputs {
    PromptInputs::new(
        prompt_text(raw),
        agent(),
        conversation(),
        0,
        timestamp(at),
        PromptSections::default(),
    )
    .expect("inputs")
}
fn cache(root: &TestRoot, name: &str) -> CacheRoot {
    let directory = root.0.join(name);
    fs::create_dir(&directory).expect("cache directory");
    CacheRoot::new(&fs::canonicalize(directory).expect("canonical cache")).expect("cache")
}
fn fixture_port() -> (TestRoot, GitMemFs, AgentId) {
    let root = TestRoot::new();
    let port = GitMemFs::new(root.0.clone()).expect("port");
    let agent = agent();
    (root, port, agent)
}
fn compiler<'a>(port: &'a GitMemFs, renders: &'a AtomicUsize) -> PromptCompiler<'a> {
    PromptCompiler::with_render_counter(port, renders)
}

mod cache {
    use super::*;

    async fn setup() -> (TestRoot, GitMemFs, CacheRoot, AtomicUsize) {
        let (root, port, agent) = fixture_port();
        port.initialize(&agent, &empty()).await.expect("initialize");
        let cache = super::cache(&root, "conversation-cache");
        (root, port, cache, AtomicUsize::new(0))
    }

    #[tokio::test]
    async fn unchanged_pair_reuses() {
        let (_root, port, cache, renders) = setup().await;
        let compiler = compiler(&port, &renders);
        let first = cache
            .get_or_compile(
                &compiler,
                &inputs("raw", "2000-01-01T00:00:00Z"),
                DeliveryCapability::RequestBoundaryOnly,
                CancellationToken::new(),
            )
            .await
            .expect("first");
        let second = cache
            .get_or_compile(
                &compiler,
                &inputs("raw", "2001-01-01T00:00:00Z"),
                DeliveryCapability::RequestBoundaryOnly,
                CancellationToken::new(),
            )
            .await
            .expect("second");
        assert!(first.rendered);
        assert!(!second.rendered);
        assert_eq!(renders.load(Ordering::SeqCst), 1);
        assert_eq!(first.persisted, second.persisted);
    }

    #[tokio::test]
    async fn changed_hash_recompiles() {
        let (_root, port, cache, renders) = setup().await;
        let compiler = compiler(&port, &renders);
        for raw in ["raw-one", "raw-two"] {
            cache
                .get_or_compile(
                    &compiler,
                    &inputs(raw, "2000-01-01T00:00:00Z"),
                    DeliveryCapability::MidConversationSystem,
                    CancellationToken::new(),
                )
                .await
                .expect("compile");
        }
        assert_eq!(renders.load(Ordering::SeqCst), 2);
        assert!(
            cache
                .load()
                .expect("load")
                .expect("record")
                .content
                .starts_with("raw-two")
        );
    }

    #[tokio::test]
    async fn changed_revision_recompiles() {
        let (_root, port, cache, renders) = setup().await;
        let compiler = compiler(&port, &renders);
        cache
            .get_or_compile(
                &compiler,
                &inputs("raw", "2000-01-01T00:00:00Z"),
                DeliveryCapability::RequestBoundaryOnly,
                CancellationToken::new(),
            )
            .await
            .expect("first");
        port.write(&agent(), &path("system/persona.md"), &valid("changed"))
            .await
            .expect("write");
        port.commit(&agent(), &message("changed"))
            .await
            .expect("commit");
        cache
            .get_or_compile(
                &compiler,
                &inputs("raw", "2001-01-01T00:00:00Z"),
                DeliveryCapability::RequestBoundaryOnly,
                CancellationToken::new(),
            )
            .await
            .expect("second");
        assert_eq!(renders.load(Ordering::SeqCst), 2);
    }
}

#[tokio::test]
async fn render_shape_matches_pinned_typescript() {
    let (_root, port, agent) = fixture_port();
    port.initialize(&agent, &empty()).await.expect("initialize");
    let files = [
        (
            "system/persona.md",
            "---\ndescription: persona\n---\nI am me.\n",
        ),
        (
            "system/nested/context.md",
            "---\ndescription: A & B <safe>\n---\nNested.\n",
        ),
        (
            "reference/z.md",
            "---\ndescription: external\n---\nIgnored body.\n",
        ),
        (
            "reference/a.md",
            "---\ndescription: external\n---\nIgnored body.\n",
        ),
        (
            "skills/hidden/SKILL.md",
            "---\ndescription: hidden\n---\nHidden.\n",
        ),
        ("notes.txt", "not markdown"),
    ];
    for (name, value) in files {
        port.write(&agent, &path(name), &content(value))
            .await
            .expect("write");
    }
    port.commit(&agent, &message("shape"))
        .await
        .expect("commit");
    let skills = vec![
        PromptSkill::new("zeta".into(), "Z skill\nmore".into(), None).expect("skill"),
        PromptSkill::new("alpha".into(), "A & < skill".into(), None).expect("skill"),
    ];
    let input = PromptInputs::new(
        prompt_text("Managed"),
        agent.clone(),
        conversation(),
        7,
        timestamp("2000-01-01T13:02:03Z"),
        PromptSections {
            runtime_reminders: vec![prompt_text("Reminder B"), prompt_text("Reminder A")],
            tool_guidance: vec![prompt_text("Tool B"), prompt_text("Tool A")],
            model_guidance: vec![prompt_text("Model B"), prompt_text("Model A")],
            skills,
        },
    )
    .expect("input");
    let record = PromptCompiler::new(&port)
        .compile(&input, CancellationToken::new())
        .await
        .expect("compile");
    let core = &record.core_memory;
    assert!(core.contains(
        "<self>\n<projection>${MEMORY_DIR}/system/persona.md</projection>\nI am me.\n</self>"
    ));
    assert!(core.contains("<nested>\n  <context>"));
    assert!(core.contains("<description>A & B <safe></description>"));
    assert!(core.contains("${MEMORY_DIR}/\n└── reference/\n    ├── a.md\n    └── z.md"));
    assert!(!core.contains("skills/hidden"));
    assert!(core.contains("System prompt last recompiled: 2000-01-01 01:02:03 PM UTC+0000"));
    assert!(record.content.starts_with("Managed\n\n"));
    assert!(record.content.contains(core));
    assert!(record.content.contains("<available_skills>\n${MEMORY_DIR}"));
    assert!(
        record.content.find("alpha").expect("alpha") < record.content.find("zeta").expect("zeta")
    );
    assert!(
        record.content.find("Reminder A").expect("A")
            < record.content.find("Reminder B").expect("B")
    );
    assert_replacement_and_missing_repo(&port).await;
}

async fn assert_replacement_and_missing_repo(port: &GitMemFs) {
    let replaced = PromptCompiler::new(port)
        .compile(
            &inputs(
                "before {CORE_MEMORY} after {CORE_MEMORY}",
                "2000-01-01T00:00:00Z",
            ),
            CancellationToken::new(),
        )
        .await
        .expect("replace");
    assert_eq!(replaced.content.matches("<memory_metadata>").count(), 2);
    let missing_input = PromptInputs::new(
        prompt_text("missing"),
        AgentId::accept("missing-agent").expect("agent"),
        conversation(),
        0,
        timestamp("2000-01-01T00:00:00Z"),
        PromptSections::default(),
    )
    .expect("missing input");
    let missing = PromptCompiler::new(port)
        .compile(&missing_input, CancellationToken::new())
        .await
        .expect("missing repo");
    assert!(missing.memfs_revision.is_none());
    assert!(missing.core_memory.starts_with("<memory_metadata>"));
}

#[test]
fn aggregate_inputs_enforce_single_budget() {
    let overhead = super::input::PROMPT_COMPILED_BYTES_MAX - 4_096;
    let below = PromptInputs::new(
        prompt_text("x"),
        agent(),
        conversation(),
        0,
        timestamp("2000-01-01T00:00:00Z"),
        PromptSections {
            runtime_reminders: vec![PromptText::new("a".repeat(overhead - 34)).expect("below")],
            ..PromptSections::default()
        },
    );
    assert!(below.is_ok());
    let above = PromptInputs::new(
        prompt_text("x"),
        agent(),
        conversation(),
        0,
        timestamp("2000-01-01T00:00:00Z"),
        PromptSections {
            runtime_reminders: vec![PromptText::new("a".repeat(overhead)).expect("fragment")],
            ..PromptSections::default()
        },
    );
    assert!(matches!(
        above,
        Err(lotta_runtime::RuntimeError::LimitExceeded { .. })
    ));
}

#[tokio::test]
async fn pre_cancel_has_no_port_cache_or_render_work() {
    let (root, port, agent) = fixture_port();
    port.initialize(&agent, &empty()).await.expect("initialize");
    let cache = cache(&root, "pre-cancel");
    let renders = AtomicUsize::new(0);
    let token = CancellationToken::new();
    token.cancel();
    let result = cache
        .get_or_compile(
            &compiler(&port, &renders),
            &inputs("raw", "2000-01-01T00:00:00Z"),
            DeliveryCapability::RequestBoundaryOnly,
            token,
        )
        .await;
    assert!(matches!(
        result,
        Err(lotta_runtime::RuntimeError::Cancelled { .. })
    ));
    assert_eq!(renders.load(Ordering::SeqCst), 0);
    assert!(cache.load().expect("cache load").is_none());
}

#[cfg(unix)]
#[test]
fn cache_final_symlink_is_never_followed() {
    use std::os::unix::fs::symlink;
    let root = TestRoot::new();
    let outside = root.0.join("outside.json");
    fs::write(&outside, b"sentinel").expect("outside");
    let directory = root.0.join("symlink-cache");
    fs::create_dir(&directory).expect("cache directory");
    symlink(&outside, directory.join("system-prompt.json")).expect("symlink");
    let cache = CacheRoot::new(&fs::canonicalize(&directory).expect("canonical")).expect("cache");
    assert!(matches!(
        cache.load(),
        Err(lotta_runtime::RuntimeError::InvalidData { .. })
    ));
    assert_eq!(fs::read(&outside).expect("outside read"), b"sentinel");
}
