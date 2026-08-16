//! Bounded parser for the baseline's simple skill frontmatter.

use super::{SkillError, limits};

/// Parsed skill document with normalized complete instructions and body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedSkillDocument {
    /// Optional explicit identifier.
    pub id: Option<String>,
    /// Optional explicit name (or compatible title fallback).
    pub name: Option<String>,
    /// Optional explicit description.
    pub description: Option<String>,
    /// Complete normalized instructions, including frontmatter.
    pub instructions: String,
    /// Normalized body without frontmatter.
    pub body: String,
}

/// Parses bounded UTF-8 skill instructions and derives metadata fallbacks.
///
/// # Errors
/// Returns a fixed error for malformed or overbound input.
pub fn parse_skill_document(
    bytes: &[u8],
    relative_directory: &str,
) -> Result<(ParsedSkillDocument, String, String, String), SkillError> {
    if bytes.len() > limits::SKILL_DOCUMENT_BYTES_MAX {
        return Err(SkillError::LimitExceeded);
    }
    let raw = std::str::from_utf8(bytes).map_err(|_| SkillError::Malformed)?;
    let normalized = normalize(raw)?;
    let (fields, body) = split_frontmatter(&normalized)?;
    let body = body.to_owned();
    let explicit_id = nonempty(fields.id);
    let explicit_name = nonempty(fields.name).or_else(|| nonempty(fields.title));
    let explicit_description = nonempty(fields.description);
    let id = explicit_id
        .clone()
        .unwrap_or_else(|| relative_directory.to_owned());
    validate_id(&id)?;
    let name = explicit_name.clone().unwrap_or_else(|| fallback_name(&id));
    validate_name(&name)?;
    let description = explicit_description.clone().unwrap_or_else(|| {
        first_paragraph(&body).unwrap_or_else(|| "No description available".into())
    });
    let document = ParsedSkillDocument {
        id: explicit_id,
        name: explicit_name,
        description: explicit_description,
        instructions: normalized,
        body,
    };
    Ok((document, id, name, description.trim().to_owned()))
}

#[derive(Default)]
struct Fields {
    id: Option<String>,
    name: Option<String>,
    title: Option<String>,
    description: Option<String>,
}

fn normalize(raw: &str) -> Result<String, SkillError> {
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let mut output = String::new();
    output
        .try_reserve(raw.len())
        .map_err(|_| SkillError::LimitExceeded)?;
    let mut chars = raw.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\0' {
            return Err(SkillError::Malformed);
        }
        if character == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            output.push('\n');
        } else {
            output.push(character);
        }
    }
    Ok(output)
}

fn split_frontmatter(value: &str) -> Result<(Fields, &str), SkillError> {
    if !value.starts_with("---\n") {
        return Ok((Fields::default(), value.trim()));
    }
    let rest = &value[4..];
    let Some(end) = rest.find("\n---") else {
        return Err(SkillError::Malformed);
    };
    let suffix = &rest[end + 4..];
    if !suffix.is_empty() && !suffix.starts_with('\n') {
        return Err(SkillError::Malformed);
    }
    let fields = parse_fields(&rest[..end])?;
    Ok((fields, suffix.strip_prefix('\n').unwrap_or(suffix).trim()))
}

fn parse_fields(value: &str) -> Result<Fields, SkillError> {
    if value.len() > limits::SKILL_FRONTMATTER_BYTES_MAX {
        return Err(SkillError::LimitExceeded);
    }
    let mut fields = Fields::default();
    for line in value.lines() {
        if line.len() > limits::SKILL_FRONTMATTER_LINE_BYTES_MAX {
            return Err(SkillError::LimitExceeded);
        }
        let Some((key, raw)) = line.split_once(':') else {
            continue;
        };
        let value = unquote(raw.trim());
        match key.trim() {
            "id" => fields.id = Some(value),
            "name" => fields.name = Some(value),
            "title" => fields.title = Some(value),
            "description" => fields.description = Some(value),
            _ => {}
        }
    }
    Ok(fields)
}

fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\'')
            || (bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"'))
    {
        value[1..value.len() - 1].to_owned()
    } else {
        value.to_owned()
    }
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    })
}

fn first_paragraph(body: &str) -> Option<String> {
    let paragraph = body.trim().split("\n\n").next().unwrap_or_default();
    (!paragraph.is_empty()).then(|| paragraph.to_owned())
}

fn fallback_name(id: &str) -> String {
    let leaf = id.rsplit('/').next().unwrap_or(id).replace('-', " ");
    let mut output = String::with_capacity(leaf.len());
    let mut word_start = true;
    for character in leaf.chars() {
        if word_start {
            output.extend(character.to_uppercase());
        } else {
            output.push(character);
        }
        word_start = character.is_whitespace();
    }
    output
}

fn validate_id(value: &str) -> Result<(), SkillError> {
    if value.is_empty() || value.len() > limits::SKILL_ID_BYTES_MAX || value.contains(['\0', '\\'])
    {
        return Err(SkillError::Malformed);
    }
    for part in value.split('/') {
        if part.is_empty() || matches!(part, "." | "..") || part.chars().any(char::is_control) {
            return Err(SkillError::Malformed);
        }
    }
    Ok(())
}

fn validate_name(value: &str) -> Result<(), SkillError> {
    if value.is_empty()
        || value.len() > limits::SKILL_NAME_BYTES_MAX
        || value
            .chars()
            .any(|character| character == '\0' || character.is_control())
    {
        Err(SkillError::Malformed)
    } else {
        Ok(())
    }
}
