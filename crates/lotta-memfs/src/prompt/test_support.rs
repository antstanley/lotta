use super::*;
use crate::GitMemFs;
use crate::tests::{TestRoot, empty};
use lotta_domain::{AgentId, ConversationId, Timestamp};
use lotta_runtime::ports::MemFsPort;
use std::fs;
use std::sync::atomic::AtomicUsize;

pub(crate) fn timestamp(value: &str) -> Timestamp {
    Timestamp::parse_persisted_rfc3339(value).expect("timestamp")
}
pub(crate) fn prompt_text(value: &str) -> PromptText {
    PromptText::new(value.to_owned()).expect("prompt text")
}
pub(crate) fn agent() -> AgentId {
    AgentId::accept("agent-local-prompt").expect("agent")
}
pub(crate) fn conversation() -> ConversationId {
    ConversationId::accept("conversation-prompt").expect("conversation")
}
pub(crate) fn inputs(raw: &str, at: &str) -> PromptInputs {
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
pub(crate) fn cache(root: &TestRoot, name: &str) -> CacheRoot {
    let directory = root.0.join(name);
    fs::create_dir(&directory).expect("cache directory");
    CacheRoot::new(&fs::canonicalize(directory).expect("canonical cache")).expect("cache")
}
pub(crate) async fn setup() -> (TestRoot, GitMemFs, AgentId) {
    let root = TestRoot::new();
    let port = GitMemFs::new(root.0.clone()).expect("port");
    let agent = agent();
    port.initialize(&agent, &empty()).await.expect("initialize");
    (root, port, agent)
}
pub(crate) fn compiler<'a>(port: &'a GitMemFs, renders: &'a AtomicUsize) -> PromptCompiler<'a> {
    PromptCompiler::with_render_counter(port, renders)
}
