use super::load::LoadedTranscript;
use crate::LocalStore;
use lotta_domain::{AgentId, ConversationId, LocalMessageRole};
use lotta_testkit::fixtures::FixtureLoader;
use lotta_testkit::roots::TemporaryRoot;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const KEY: &str = "ZGVmYXVsdDphZ2VudC1sb2NhbC1maXh0dXJl";

pub(crate) struct CorpusStore {
    pub(crate) root: TemporaryRoot,
    pub(crate) store: LocalStore,
    pub(crate) agent: AgentId,
    pub(crate) conversation: ConversationId,
    pub(crate) directory: PathBuf,
}

impl CorpusStore {
    pub(crate) fn messages(&self) -> PathBuf {
        self.directory.join("messages.jsonl")
    }

    pub(crate) fn manifest(&self) -> PathBuf {
        self.directory.join("manifest.json")
    }

    pub(crate) fn conversation_path(&self) -> PathBuf {
        self.directory.join("conversation.json")
    }
}

pub(crate) fn corpus(case: &str, suffix: &str, label: &str) -> CorpusStore {
    let root = TemporaryRoot::new(label).expect("temporary root");
    let backend = root.path().join("local-backend");
    copy_fixture(case, suffix, &backend);
    let paths = crate::StorePaths::new(&backend).expect("store paths");
    let directory = backend.join("conversations").join(KEY);
    CorpusStore {
        root,
        store: LocalStore::new(paths),
        agent: AgentId::accept("agent-local-fixture").expect("agent"),
        conversation: ConversationId::default_for_agent(),
        directory,
    }
}

fn copy_fixture(case: &str, suffix: &str, target: &Path) {
    let loader = FixtureLoader::new();
    let source = format!("persistence/{case}/{suffix}");
    for relative in loader.list_tree(&source).expect("fixture tree") {
        let bytes = loader
            .load_bytes(format!("{source}/{relative}"))
            .expect("fixture bytes");
        let destination = target.join(&relative);
        std::fs::create_dir_all(destination.parent().expect("parent")).expect("directory");
        std::fs::write(destination, bytes).expect("fixture copy");
    }
}

pub(crate) fn snapshot(path: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut output = BTreeMap::new();
    snapshot_into(path, path, &mut output);
    output
}

fn snapshot_into(root: &Path, current: &Path, output: &mut BTreeMap<String, Vec<u8>>) {
    for entry in std::fs::read_dir(current).expect("read directory") {
        let entry = entry.expect("entry");
        let path = entry.path();
        if entry.file_type().expect("type").is_dir() {
            snapshot_into(root, &path, output);
        } else {
            let relative = path
                .strip_prefix(root)
                .expect("relative")
                .to_string_lossy()
                .into_owned();
            output.insert(relative, std::fs::read(path).expect("bytes"));
        }
    }
}

pub(crate) fn ids(loaded: &LoadedTranscript) -> Vec<String> {
    loaded
        .messages()
        .iter()
        .map(|message| message.id.as_str().to_owned())
        .collect()
}

pub(crate) fn message_text(loaded: &LoadedTranscript, id: &str) -> String {
    let message = loaded
        .messages()
        .iter()
        .find(|message| message.id.as_str() == id)
        .expect("message id");
    message
        .content
        .as_ref()
        .and_then(|content| content.as_value().as_array())
        .and_then(|parts| parts.first())
        .and_then(|part| part.get("text"))
        .and_then(Value::as_str)
        .expect("message text")
        .to_owned()
}

pub(crate) fn tool_result_text(loaded: &LoadedTranscript) -> String {
    let message = loaded
        .messages()
        .iter()
        .find(|message| message.role == LocalMessageRole::ToolResult)
        .expect("tool result");
    message
        .content
        .as_ref()
        .and_then(|content| content.as_value().as_array())
        .and_then(|parts| parts.first())
        .and_then(|part| part.get("text"))
        .and_then(Value::as_str)
        .expect("tool result text")
        .to_owned()
}

pub(crate) fn directory_names(path: &Path) -> BTreeSet<String> {
    std::fs::read_dir(path)
        .expect("listing")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

pub(crate) fn json_lines(path: &Path) -> Vec<Value> {
    String::from_utf8(std::fs::read(path).expect("messages bytes"))
        .expect("UTF-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("JSON row"))
        .collect()
}

pub(crate) fn write_json_lines(path: &Path, rows: &[Value], trailing_lf: bool) {
    let mut text = rows
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    if trailing_lf {
        text.push('\n');
    }
    std::fs::write(path, text).expect("write rows");
}
