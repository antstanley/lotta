use super::CompiledPromptRecord;
use super::compile::hash_raw_system;
use super::input::{
    PROMPT_COMPILED_BYTES_MAX, PromptInputs, PromptText, normalize_path, validate_normalized_path,
    validate_tag_label, validate_tree_name,
};
use super::skills::render_skills;
use super::util::{
    BoundedString, append_spaces, checked_add, checked_budget, invalid, limit, reserve_one,
};
use lotta_runtime::RuntimeError;
use lotta_runtime::boundary::{MemoryFileContent, RepositoryPath, RevisionId};
use std::collections::BTreeMap;
use std::fmt::Write as _;

const CORE_MEMORY_VARIABLE: &str = "{CORE_MEMORY}";

pub(crate) struct MemoryFile {
    relative_path: String,
    label: String,
    value: String,
    description: String,
}

impl MemoryFile {
    pub(crate) fn label(&self) -> &str {
        &self.label
    }
}

impl MemoryFile {
    pub(crate) fn parse(
        path: &RepositoryPath,
        content: &MemoryFileContent,
    ) -> Result<Self, RuntimeError> {
        if path.as_path().extension().is_none_or(|value| value != "md") {
            return Err(invalid("prompt memory Markdown"));
        }
        let source = path
            .as_path()
            .to_str()
            .ok_or_else(|| invalid("prompt memory path"))?;
        validate_normalized_path(source)?;
        let relative_path = normalize_path(source)?;
        let text =
            std::str::from_utf8(content.as_slice()).map_err(|_| invalid("prompt memory UTF-8"))?;
        let (description, value) = parse_frontmatter(text)?;
        let label = relative_path
            .strip_suffix(".md")
            .ok_or_else(|| invalid("prompt memory label"))?
            .to_owned();
        validate_tag_label(&label)?;
        Ok(Self {
            relative_path,
            label,
            value,
            description,
        })
    }

    pub(crate) fn retained_bytes(&self) -> Result<usize, RuntimeError> {
        let mut total = checked_add(
            self.relative_path.len(),
            self.label.len(),
            "prompt memory bytes",
        )?;
        total = checked_add(total, self.value.len(), "prompt memory bytes")?;
        checked_add(total, self.description.len(), "prompt memory bytes")
    }
}

pub(crate) fn render_record(
    input: &PromptInputs,
    revision: Option<RevisionId>,
    files: &[MemoryFile],
) -> Result<CompiledPromptRecord, RuntimeError> {
    let projection = render_memfs(files)?;
    let metadata = render_metadata(input)?;
    let mut core = BoundedString::default();
    if !projection.trim().is_empty() {
        core.push(&projection)?;
        core.push("\n\n")?;
    }
    core.push(&metadata)?;
    let core_memory = core.finish();
    let mut content = inject_core(input.raw_system().as_str(), &core_memory)?;
    append_sections(&mut content, input)?;
    Ok(CompiledPromptRecord {
        content,
        core_memory,
        mid_conversation_system_prompt: None,
        compiled_at: input.compiled_at(),
        raw_system_hash: hash_raw_system(input.raw_system().as_str()),
        memfs_revision: revision.map(RevisionId::into_string),
    })
}

pub(crate) fn parse_frontmatter(text: &str) -> Result<(String, String), RuntimeError> {
    if !text.starts_with("---\n") {
        return Err(invalid("prompt memory frontmatter"));
    }
    let rest = &text[4..];
    let end = rest
        .find("\n---\n")
        .ok_or_else(|| invalid("prompt memory frontmatter"))?;
    let header = &rest[..end];
    let body = &rest[end + 5..];
    let mut description = String::new();
    for line in header.lines() {
        if let Some(value) = line.strip_prefix("description:") {
            description = parse_description(value.trim())?;
        }
    }
    Ok((description.trim().to_owned(), body.to_owned()))
}

fn parse_description(value: &str) -> Result<String, RuntimeError> {
    if value.starts_with('"') {
        serde_json::from_str(value).map_err(|_| invalid("prompt description"))
    } else {
        Ok(value.to_owned())
    }
}

pub(crate) fn render_memfs(files: &[MemoryFile]) -> Result<String, RuntimeError> {
    if files.is_empty() {
        return Ok(String::new());
    }
    let mut out = BoundedString::default();
    out.line("Reminder: <projection> contains the local path of the memory file projection.")?;
    if let Some(persona) = files.iter().find(|file| file.label == "system/persona") {
        out.push("\n\n<self>\n<projection>${MEMORY_DIR}/system/persona.md</projection>\n")?;
        out.push(persona.value.trim_end())?;
        out.push("\n</self>")?;
    }
    let mut system = Vec::new();
    let mut external = Vec::new();
    for file in files {
        if file.label.starts_with("system/") && file.label != "system/persona" {
            reserve_one(&mut system, "prompt system indexes")?;
            system.push(file);
        } else if !file.label.starts_with("skills/") && !file.label.starts_with("system/") {
            reserve_one(&mut external, "prompt external indexes")?;
            external.push(file);
        }
    }
    if !system.is_empty() || !external.is_empty() {
        out.push("\n\n<memory>\n")?;
        render_system_tree(&system, &mut out)?;
        if !system.is_empty() && !external.is_empty() {
            out.push("\n")?;
        }
        if !external.is_empty() {
            render_external_tree(&external, &mut out)?;
        }
        out.push("\n</memory>")?;
    }
    Ok(out.finish())
}

#[derive(Default)]
struct SystemNode<'a> {
    children: BTreeMap<String, SystemNode<'a>>,
    file: Option<&'a MemoryFile>,
}

fn render_system_tree(files: &[&MemoryFile], out: &mut BoundedString) -> Result<(), RuntimeError> {
    let mut root = SystemNode::default();
    for file in files {
        let label = file
            .label
            .strip_prefix("system/")
            .ok_or_else(|| invalid("system memory label"))?;
        let mut node = &mut root;
        for part in label.split('/') {
            node = node.children.entry(part.to_owned()).or_default();
        }
        node.file = Some(file);
    }
    let mut path = Vec::new();
    render_system_node(&root, 0, &mut path, out)
}

fn render_system_node(
    node: &SystemNode<'_>,
    indent: usize,
    path: &mut Vec<String>,
    out: &mut BoundedString,
) -> Result<(), RuntimeError> {
    let pad_bytes = indent
        .checked_mul(2)
        .ok_or_else(|| limit("prompt indentation"))?;
    for (name, child) in &node.children {
        append_spaces(out, pad_bytes)?;
        out.push("<")?;
        out.push(name)?;
        out.push(">\n")?;
        reserve_one(path, "prompt system path")?;
        path.push(name.clone());
        if let Some(file) = child.file {
            append_spaces(out, checked_add(pad_bytes, 2, "prompt indentation")?)?;
            out.push("<projection>${MEMORY_DIR}/system/")?;
            append_path(out, path)?;
            out.push(".md</projection>\n")?;
            if !file.description.trim().is_empty() {
                append_spaces(out, checked_add(pad_bytes, 2, "prompt indentation")?)?;
                out.push("<description>")?;
                out.push(file.description.trim())?;
                out.push("</description>\n")?;
            }
            if !file.value.trim_end().is_empty() {
                append_spaces(out, checked_add(pad_bytes, 2, "prompt indentation")?)?;
                out.push(file.value.trim_end())?;
                out.push("\n")?;
            }
        }
        render_system_node(child, checked_add(indent, 1, "prompt depth")?, path, out)?;
        drop(path.pop());
        append_spaces(out, pad_bytes)?;
        out.push("</")?;
        out.push(name)?;
        out.push(">\n")?;
    }
    if !out.value.is_empty() && out.value.ends_with('\n') {
        out.value.pop();
    }
    Ok(())
}

fn append_path(out: &mut BoundedString, path: &[String]) -> Result<(), RuntimeError> {
    for (index, part) in path.iter().enumerate() {
        if index != 0 {
            out.push("/")?;
        }
        out.push(part)?;
    }
    Ok(())
}

#[derive(Default)]
struct ExternalNode {
    children: BTreeMap<String, ExternalNode>,
    file: bool,
}

fn render_external_tree(
    files: &[&MemoryFile],
    out: &mut BoundedString,
) -> Result<(), RuntimeError> {
    let mut root = ExternalNode::default();
    for file in files {
        let mut node = &mut root;
        for part in file.relative_path.split('/') {
            validate_tree_name(part)?;
            node = node.children.entry(part.to_owned()).or_default();
        }
        node.file = true;
    }
    out.push("<external_projection>\n${MEMORY_DIR}/\n")?;
    render_external_node(&root, "", out)?;
    out.push("</external_projection>")
}

fn render_external_node(
    node: &ExternalNode,
    prefix: &str,
    out: &mut BoundedString,
) -> Result<(), RuntimeError> {
    let mut entries = Vec::new();
    entries
        .try_reserve(node.children.len())
        .map_err(|_| limit("prompt tree entries"))?;
    for entry in &node.children {
        entries.push(entry);
    }
    entries.sort_by(|left, right| {
        left.1
            .file
            .cmp(&right.1.file)
            .then_with(|| left.0.cmp(right.0))
    });
    let last = entries.len().checked_sub(1);
    for (index, (name, child)) in entries.into_iter().enumerate() {
        let final_entry = Some(index) == last;
        out.push(prefix)?;
        out.push(if final_entry {
            "└── "
        } else {
            "├── "
        })?;
        out.push(name)?;
        if !child.file {
            out.push("/")?;
        }
        out.push("\n")?;
        if !child.file {
            let continuation = if final_entry { "    " } else { "│   " };
            let mut next = BoundedString::default();
            next.push(prefix)?;
            next.push(continuation)?;
            render_external_node(child, &next.finish(), out)?;
        }
    }
    Ok(())
}

fn render_metadata(input: &PromptInputs) -> Result<String, RuntimeError> {
    let display = input
        .compiled_at()
        .as_utc()
        .format("%Y-%m-%d %I:%M:%S %p UTC+0000");
    let mut out = BoundedString::default();
    out.push("<memory_metadata>\n- AGENT_ID: ")?;
    out.push(input.agent_id().as_str())?;
    out.push("\n- CONVERSATION_ID: ")?;
    out.push(input.conversation_id().as_str())?;
    out.push("\n- System prompt last recompiled: ")?;
    write!(&mut out.value, "{display}").map_err(|_| limit("compiled prompt bytes"))?;
    out.push("\n- ")?;
    write!(&mut out.value, "{}", input.previous_message_count())
        .map_err(|_| limit("compiled prompt bytes"))?;
    out.push(" previous messages between you and the user are stored in recall memory\n")?;
    out.push("</memory_metadata>")?;
    if out.value.len() > PROMPT_COMPILED_BYTES_MAX {
        return Err(limit("compiled prompt bytes"));
    }
    Ok(out.finish())
}

pub(crate) fn inject_core_with_limit(
    raw: &str,
    core: &str,
    limit_bytes: usize,
) -> Result<String, RuntimeError> {
    let mut out = BoundedString::with_limit(limit_bytes);
    let mut cursor = 0usize;
    let mut found = false;
    for (offset, _) in raw.match_indices(CORE_MEMORY_VARIABLE) {
        found = true;
        out.push(&raw[cursor..offset])?;
        out.push(core)?;
        cursor = checked_add(offset, CORE_MEMORY_VARIABLE.len(), "compiled prompt bytes")?;
    }
    if found {
        out.push(&raw[cursor..])?;
    } else {
        out.push(raw.trim_end())?;
        out.push("\n\n")?;
        out.push(core)?;
    }
    Ok(out.finish())
}

fn inject_core(raw: &str, core: &str) -> Result<String, RuntimeError> {
    inject_core_with_limit(raw, core, PROMPT_COMPILED_BYTES_MAX)
}

fn append_sections(content: &mut String, input: &PromptInputs) -> Result<(), RuntimeError> {
    append_text_section(content, "runtime_reminders", input.runtime_reminders())?;
    append_text_section(content, "tool_guidance", input.tool_guidance())?;
    append_text_section(content, "model_guidance", input.model_guidance())?;
    let skills = render_skills(input.skills())?;
    if !skills.is_empty() {
        append_block(content, &skills)?;
    }
    Ok(())
}

fn append_text_section(
    content: &mut String,
    label: &str,
    values: &[PromptText],
) -> Result<(), RuntimeError> {
    if values.is_empty() {
        return Ok(());
    }
    let mut sorted = Vec::new();
    sorted
        .try_reserve(values.len())
        .map_err(|_| limit("prompt section refs"))?;
    for value in values {
        sorted.push(value.as_str());
    }
    sorted.sort_unstable();
    let mut block = BoundedString::default();
    block.push("<")?;
    block.push(label)?;
    block.push(">\n")?;
    for (index, value) in sorted.into_iter().enumerate() {
        if index != 0 {
            block.push("\n")?;
        }
        block.push(value)?;
    }
    block.push("\n</")?;
    block.push(label)?;
    block.push(">")?;
    append_block(content, &block.finish())
}

fn append_block(content: &mut String, block: &str) -> Result<(), RuntimeError> {
    content.truncate(content.trim_end().len());
    let required = checked_add(block.trim_start().len(), 2, "compiled prompt bytes")?;
    checked_budget(content.len(), required, "compiled prompt bytes")?;
    content
        .try_reserve(required)
        .map_err(|_| limit("compiled prompt bytes"))?;
    content.push_str("\n\n");
    content.push_str(block.trim_start());
    Ok(())
}
