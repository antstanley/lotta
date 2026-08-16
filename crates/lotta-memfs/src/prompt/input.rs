use super::util::{BoundedString, checked_add, checked_budget, invalid, limit};
use lotta_domain::{AgentId, ConversationId, Timestamp};
use lotta_runtime::RuntimeError;

pub(crate) const PROMPT_COMPILED_BYTES_MAX: usize = 8 * 1024 * 1024;
const PROMPT_INPUT_ITEMS_MAX: usize = 10_000;
const PROMPT_RENDER_OVERHEAD_BYTES: usize = 4_096;

/// Validated owning prompt text input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PromptText(String);

impl PromptText {
    /// Validates one prompt fragment under the compilation input ceiling.
    ///
    /// # Errors
    /// Returns a limit error when the fragment exceeds the prompt ceiling.
    pub fn new(value: String) -> Result<Self, RuntimeError> {
        if value.len() > PROMPT_COMPILED_BYTES_MAX {
            return Err(limit("prompt input bytes"));
        }
        Ok(Self(value))
    }

    /// Borrows the original text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One selected skill rendered deterministically after core memory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PromptSkill {
    name: PromptText,
    description: PromptText,
    location: Option<PromptText>,
}

impl PromptSkill {
    /// Validates one selected skill and optional display-only location.
    ///
    /// # Errors
    /// Rejects structurally unsafe names, locations, or oversized text.
    pub fn new(
        name: String,
        description: String,
        location: Option<String>,
    ) -> Result<Self, RuntimeError> {
        validate_skill_name(&name)?;
        if let Some(value) = location.as_deref() {
            validate_display_path(value)?;
        }
        Ok(Self {
            name: PromptText::new(name)?,
            description: PromptText::new(description)?,
            location: location.map(PromptText::new).transpose()?,
        })
    }

    /// Returns the full stable skill name.
    #[must_use]
    pub fn name(&self) -> &PromptText {
        &self.name
    }

    /// Returns the supplied description.
    #[must_use]
    pub fn description(&self) -> &PromptText {
        &self.description
    }

    /// Returns the optional display-only location.
    #[must_use]
    pub fn location(&self) -> Option<&PromptText> {
        self.location.as_ref()
    }
}

/// Complete explicit compilation inputs; compilation performs no ambient reads.
pub struct PromptInputs {
    raw_system: PromptText,
    agent_id: AgentId,
    conversation_id: ConversationId,
    previous_message_count: usize,
    compiled_at: Timestamp,
    runtime_reminders: Vec<PromptText>,
    tool_guidance: Vec<PromptText>,
    model_guidance: Vec<PromptText>,
    skills: Vec<PromptSkill>,
}

/// Grouped optional prompt sections kept explicit and deterministic.
#[derive(Default)]
pub struct PromptSections {
    /// Runtime-generated reminders.
    pub runtime_reminders: Vec<PromptText>,
    /// Tool guidance fragments.
    pub tool_guidance: Vec<PromptText>,
    /// Model guidance fragments.
    pub model_guidance: Vec<PromptText>,
    /// Selected skills.
    pub skills: Vec<PromptSkill>,
}

impl PromptInputs {
    /// Validates and owns every explicit compilation input.
    ///
    /// # Errors
    /// Rejects aggregate input limits and invalid selected skills.
    pub fn new(
        raw_system: PromptText,
        agent_id: AgentId,
        conversation_id: ConversationId,
        previous_message_count: usize,
        compiled_at: Timestamp,
        sections: PromptSections,
    ) -> Result<Self, RuntimeError> {
        let value = Self {
            raw_system,
            agent_id,
            conversation_id,
            previous_message_count,
            compiled_at,
            runtime_reminders: sections.runtime_reminders,
            tool_guidance: sections.tool_guidance,
            model_guidance: sections.model_guidance,
            skills: sections.skills,
        };
        validate_inputs(&value)?;
        Ok(value)
    }

    /// Returns the raw managed system prompt.
    #[must_use]
    pub fn raw_system(&self) -> &PromptText {
        &self.raw_system
    }
    /// Returns the memory owner.
    #[must_use]
    pub fn agent_id(&self) -> &AgentId {
        &self.agent_id
    }
    /// Returns the represented conversation.
    #[must_use]
    pub fn conversation_id(&self) -> &ConversationId {
        &self.conversation_id
    }
    /// Returns the prior message count.
    #[must_use]
    pub const fn previous_message_count(&self) -> usize {
        self.previous_message_count
    }
    /// Returns the injected compilation instant.
    #[must_use]
    pub const fn compiled_at(&self) -> Timestamp {
        self.compiled_at
    }
    /// Returns runtime reminders.
    #[must_use]
    pub fn runtime_reminders(&self) -> &[PromptText] {
        &self.runtime_reminders
    }
    /// Returns tool guidance.
    #[must_use]
    pub fn tool_guidance(&self) -> &[PromptText] {
        &self.tool_guidance
    }
    /// Returns model guidance.
    #[must_use]
    pub fn model_guidance(&self) -> &[PromptText] {
        &self.model_guidance
    }
    /// Returns selected skills.
    #[must_use]
    pub fn skills(&self) -> &[PromptSkill] {
        &self.skills
    }
}

pub(crate) fn validate_inputs(input: &PromptInputs) -> Result<(), RuntimeError> {
    let mut bytes = checked_budget(0, input.raw_system().as_str().len(), "prompt input bytes")?;
    for length in [
        input.runtime_reminders().len(),
        input.tool_guidance().len(),
        input.model_guidance().len(),
        input.skills().len(),
    ] {
        if length > PROMPT_INPUT_ITEMS_MAX {
            return Err(limit("prompt input items"));
        }
    }
    for value in input
        .runtime_reminders()
        .iter()
        .chain(input.tool_guidance())
        .chain(input.model_guidance())
    {
        bytes = checked_budget(
            bytes,
            checked_add(value.as_str().len(), 32, "prompt input bytes")?,
            "prompt input bytes",
        )?;
    }
    for skill in input.skills() {
        bytes = checked_budget(
            bytes,
            checked_add(skill.name().as_str().len(), 32, "prompt input bytes")?,
            "prompt input bytes",
        )?;
        bytes = checked_budget(
            bytes,
            checked_add(skill.description().as_str().len(), 32, "prompt input bytes")?,
            "prompt input bytes",
        )?;
        if let Some(location) = skill.location() {
            bytes = checked_budget(
                bytes,
                checked_add(location.as_str().len(), 16, "prompt input bytes")?,
                "prompt input bytes",
            )?;
        }
    }
    checked_budget(bytes, PROMPT_RENDER_OVERHEAD_BYTES, "prompt input bytes")?;
    Ok(())
}

pub(crate) fn normalize_path(value: &str) -> Result<String, RuntimeError> {
    let mut out = BoundedString::default();
    for character in value.chars() {
        if character == '\\' {
            out.push("/")?;
        } else {
            let mut bytes = [0u8; 4];
            out.push(character.encode_utf8(&mut bytes))?;
        }
    }
    Ok(out.finish())
}

fn validate_skill_name(value: &str) -> Result<(), RuntimeError> {
    for part in value.split('/') {
        validate_tree_name(part)?;
    }
    Ok(())
}

fn validate_display_path(value: &str) -> Result<(), RuntimeError> {
    validate_normalized_path(value)
}

pub(crate) fn validate_normalized_path(value: &str) -> Result<(), RuntimeError> {
    if value.is_empty()
        || value
            .chars()
            .any(|character| character == '\0' || character.is_control())
    {
        return Err(invalid("prompt display path"));
    }
    let mut start = 0usize;
    let mut first = true;
    for (index, character) in value.char_indices() {
        if matches!(character, '/' | '\\') {
            validate_path_component(&value[start..index], first && index == 0)?;
            start = checked_add(index, character.len_utf8(), "prompt display path")?;
            first = false;
        }
    }
    validate_path_component(&value[start..], false)
}

fn validate_path_component(value: &str, absolute_root: bool) -> Result<(), RuntimeError> {
    if value.is_empty() {
        return if absolute_root {
            Ok(())
        } else {
            Err(invalid("prompt path component"))
        };
    }
    validate_tree_name(value)
}

pub(crate) fn validate_tag_label(label: &str) -> Result<(), RuntimeError> {
    for part in label.split('/') {
        validate_tree_name(part)?;
        if label.starts_with("system/") {
            validate_xml_name(part)?;
        }
    }
    Ok(())
}

fn validate_xml_name(value: &str) -> Result<(), RuntimeError> {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return Err(invalid("XML label"));
    };
    if !(first.is_ascii_alphabetic() || first == '_')
        || !chars.all(|value| value.is_ascii_alphanumeric() || matches!(value, '_' | '-'))
    {
        return Err(invalid("system memory XML label"));
    }
    Ok(())
}

pub(crate) fn validate_tree_name(value: &str) -> Result<(), RuntimeError> {
    if value.is_empty()
        || matches!(value, "." | "..")
        || value.contains(['/', '\\', '<', '>'])
        || value.contains("├──")
        || value.contains("└──")
        || value.contains("│")
        || value.chars().any(char::is_control)
    {
        return Err(invalid("prompt tree label"));
    }
    Ok(())
}
