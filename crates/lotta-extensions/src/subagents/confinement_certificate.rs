use super::confinement::*;
use super::types::*;
use lotta_domain::{AgentId, ConversationId};
use std::future::ready;
use std::path::PathBuf;

struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("lotta-task46-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("inside")).unwrap();
        std::fs::create_dir_all(root.join("outside")).unwrap();
        std::fs::write(root.join("inside/marker"), "inside").unwrap();
        std::fs::write(root.join("outside/marker"), "outside").unwrap();
        Self { root }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn filesystem_absolute_parent_and_symlink_escape_are_rejected() {
    let fixture = Fixture::new();
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        fixture.root.join("outside"),
        fixture.root.join("inside/link"),
    )
    .unwrap();
    let plan = ConfinementPlan::new(
        SubagentType::GeneralPurpose,
        &[fixture.root.join("inside")],
        None,
        None,
    )
    .unwrap();
    assert!(
        plan.resolve_existing(&fixture.root.join("inside/marker"))
            .is_ok()
    );
    assert!(
        plan.resolve_existing(&fixture.root.join("outside/marker"))
            .is_err()
    );
    assert!(
        plan.resolve_existing(&fixture.root.join("inside/../outside/marker"))
            .is_err()
    );
    #[cfg(unix)]
    assert!(
        plan.resolve_existing(&fixture.root.join("inside/link/marker"))
            .is_err()
    );
}

#[test]
fn reflection_primary_rejected_and_exact_worktree_allowed() {
    let fixture = Fixture::new();
    let primary = fixture.root.join("outside");
    let worktree = fixture.root.join("inside");
    let scope = MemoryScope {
        primary_root: primary.clone(),
        readonly_roots: Vec::new(),
        writable_roots: vec![worktree.clone()],
    };
    let plan =
        ConfinementPlan::new(SubagentType::Reflection, &[], Some(&scope), Some(&worktree)).unwrap();
    assert_eq!(
        plan.memory_writable_roots(),
        [worktree.canonicalize().unwrap()]
    );
    assert!(plan.resolve_existing(&primary.join("marker")).is_err());
    assert!(
        plan.resolve_existing(&fixture.root.join("inside/marker"))
            .is_ok()
    );
    let direct = MemoryScope {
        writable_roots: vec![primary.clone()],
        ..scope
    };
    assert!(
        ConfinementPlan::new(SubagentType::Reflection, &[], Some(&direct), Some(&primary),)
            .is_err()
    );
}

struct History;
impl HistoricalMessagePort for History {
    type Read<'a> = std::future::Ready<Result<Vec<u8>, RequestError>>;
    fn read_history(&self, _: &AgentId, _: &ConversationId) -> Self::Read<'_> {
        ready(Ok(b"parent-history".to_vec()))
    }
}

#[tokio::test]
async fn cross_conversation_and_agent_history_are_rejected() {
    let agent = AgentId::accept("parent").unwrap();
    let conversation = ConversationId::accept("conversation").unwrap();
    let scoped = ScopedHistory::new(
        ParentScope {
            agent_id: agent.clone(),
            conversation_id: conversation.clone(),
            runtime_id: "runtime".into(),
        },
        History,
    );
    assert_eq!(
        scoped.read(&agent, &conversation).await.unwrap(),
        b"parent-history"
    );
    assert_eq!(
        scoped
            .read(&AgentId::accept("other").unwrap(), &conversation)
            .await,
        Err(RequestError::Capability)
    );
    assert_eq!(
        scoped
            .read(&agent, &ConversationId::accept("other").unwrap())
            .await,
        Err(RequestError::Capability)
    );
}
