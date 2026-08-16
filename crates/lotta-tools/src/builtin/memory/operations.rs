use super::{MemoryState, patch};
use lotta_runtime::boundary::{CommitMessage, MemoryFileContent, RepositoryPath};
use lotta_runtime::ports::{MemFsCommitAuthor, MemFsMutation, MemFsTransactionResult};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MemoryError {
    Invalid,
    Infrastructure,
}

pub(super) async fn execute(
    state: &MemoryState,
    name: &str,
    input: &Value,
) -> Result<String, MemoryError> {
    let _guard = state.mutations.lock().await;
    let reason = required(input, "reason")?;
    let message = commit_message(reason)?;
    let mutations = match name {
        "memory" => memory_mutations(state, input).await?,
        "memory_apply_patch" => patch_mutations(state, input).await?,
        _ => return Err(MemoryError::Invalid),
    };
    let author = MemFsCommitAuthor {
        name: CommitMessage::new(state.author.name().into()).map_err(|_| MemoryError::Invalid)?,
        email: CommitMessage::new(state.author.email().into()).map_err(|_| MemoryError::Invalid)?,
    };
    match state
        .port
        .transact(&state.agent, &mutations, &message, &author)
        .await
    {
        Ok(MemFsTransactionResult::Committed(revision)) => Ok(format!(
            "Memory change committed ({}).",
            &revision.as_str()[..7.min(revision.as_str().len())]
        )),
        Ok(MemFsTransactionResult::NoChange) => {
            Ok("Memory change made no effective changes; nothing was committed.".into())
        }
        Err(_) => Err(MemoryError::Infrastructure),
    }
}

async fn memory_mutations(
    state: &MemoryState,
    input: &Value,
) -> Result<Vec<MemFsMutation>, MemoryError> {
    let command = required(input, "command")?;
    match command {
        "create" => create(input),
        "str_replace" => replace(state, input).await,
        "insert" => insert(state, input).await,
        "delete" => delete(state, input).await,
        "rename" => rename(state, input).await,
        "update_description" => update_description(state, input).await,
        _ => Err(MemoryError::Invalid),
    }
}

async fn patch_mutations(
    state: &MemoryState,
    input: &Value,
) -> Result<Vec<MemFsMutation>, MemoryError> {
    let input = required(input, "input")?;
    let patch = patch::parse(input).map_err(|()| MemoryError::Invalid)?;
    patch::mutations(state.port.as_ref(), &state.agent, patch)
        .await
        .map_err(|()| MemoryError::Invalid)
}

fn create(input: &Value) -> Result<Vec<MemFsMutation>, MemoryError> {
    let path = memory_path(required(input, "file_path")?)?;
    let description = required(input, "description")?;
    if description.trim().is_empty() {
        return Err(MemoryError::Invalid);
    }
    let body = optional(input, "file_text").unwrap_or("");
    let text = render(description, None, body)?;
    Ok(vec![MemFsMutation::Write {
        path,
        contents: text,
    }])
}

async fn replace(state: &MemoryState, input: &Value) -> Result<Vec<MemFsMutation>, MemoryError> {
    let path = memory_path(required(input, "file_path")?)?;
    let file = editable(state, &path).await?;
    let old = required(input, "old_string")?;
    let new = required(input, "new_string")?;
    let index = file.body.find(old).ok_or(MemoryError::Invalid)?;
    let body = format!(
        "{}{}{}",
        &file.body[..index],
        new,
        &file.body[index + old.len()..]
    );
    write(path, &file, &body)
}

async fn insert(state: &MemoryState, input: &Value) -> Result<Vec<MemFsMutation>, MemoryError> {
    let path = memory_path(required(input, "file_path")?)?;
    let file = editable(state, &path).await?;
    let line = input
        .get("insert_line")
        .and_then(Value::as_f64)
        .ok_or(MemoryError::Invalid)?;
    if !line.is_finite() {
        return Err(MemoryError::Invalid);
    }
    let text = required(input, "insert_text")?;
    let mut lines: Vec<&str> = if file.body.is_empty() {
        Vec::new()
    } else {
        file.body.split('\n').collect()
    };
    let line = line.floor().max(1.0);
    if line > f64::from(u32::MAX) {
        return Err(MemoryError::Invalid);
    }
    let line = line
        .to_string()
        .parse::<usize>()
        .map_err(|_| MemoryError::Invalid)?;
    let index = line.saturating_sub(1).min(lines.len());
    let insertion: Vec<&str> = text.split('\n').collect();
    lines.splice(index..index, insertion);
    write(path, &file, &lines.join("\n"))
}

async fn delete(state: &MemoryState, input: &Value) -> Result<Vec<MemFsMutation>, MemoryError> {
    let path = memory_path(required(input, "file_path")?)?;
    editable(state, &path).await?;
    Ok(vec![MemFsMutation::Delete { path }])
}

async fn rename(state: &MemoryState, input: &Value) -> Result<Vec<MemFsMutation>, MemoryError> {
    let source = memory_path(required(input, "old_path")?)?;
    let target = memory_path(required(input, "new_path")?)?;
    editable(state, &source).await?;
    Ok(vec![MemFsMutation::Rename { source, target }])
}

async fn update_description(
    state: &MemoryState,
    input: &Value,
) -> Result<Vec<MemFsMutation>, MemoryError> {
    let path = memory_path(required(input, "file_path")?)?;
    let file = editable(state, &path).await?;
    let description = required(input, "description")?;
    if description.trim().is_empty() || description.contains(['\n', '\r', '\0']) {
        return Err(MemoryError::Invalid);
    }
    let replacement = format!("description: {}", description.trim());
    let frontmatter = file
        .frontmatter
        .lines()
        .map(|line| {
            if line
                .split_once(':')
                .is_some_and(|(key, _)| key.trim() == "description")
            {
                replacement.as_str()
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let contents =
        MemoryFileContent::new(format!("---\n{frontmatter}\n---\n{}", file.body).into_bytes())
            .map_err(|_| MemoryError::Invalid)?;
    validate_memory(&contents).map_err(|()| MemoryError::Invalid)?;
    Ok(vec![MemFsMutation::Write { path, contents }])
}

pub(super) struct MemoryFile {
    pub(super) read_only: bool,
    frontmatter: String,
    body: String,
}

async fn editable(state: &MemoryState, path: &RepositoryPath) -> Result<MemoryFile, MemoryError> {
    reject_symlink(&state.root, path)?;
    let contents = state
        .port
        .read(&state.agent, path)
        .await
        .map_err(|_| MemoryError::Invalid)?;
    let file = parse(contents.as_slice())?;
    if file.read_only {
        return Err(MemoryError::Invalid);
    }
    Ok(file)
}

pub(super) fn validate_memory(contents: &MemoryFileContent) -> Result<MemoryFile, ()> {
    parse(contents.as_slice()).map_err(|_| ())
}

fn parse(bytes: &[u8]) -> Result<MemoryFile, MemoryError> {
    let text = std::str::from_utf8(bytes).map_err(|_| MemoryError::Invalid)?;
    let rest = text.strip_prefix("---\n").ok_or(MemoryError::Invalid)?;
    let (frontmatter, body) = rest.split_once("\n---\n").ok_or(MemoryError::Invalid)?;
    let mut description = None;
    let mut read_only = None;
    let mut limit = false;
    for line in frontmatter.lines() {
        if line.is_empty() || line.starts_with([' ', '\t']) {
            continue;
        }
        let (key, value) = line.split_once(':').ok_or(MemoryError::Invalid)?;
        match key.trim() {
            "description" => set_once(&mut description, parse_description(value)?)?,
            "read_only" => set_once(&mut read_only, parse_bool(value)?)?,
            "limit" if !limit => limit = true,
            _ => return Err(MemoryError::Invalid),
        }
    }
    description.ok_or(MemoryError::Invalid)?;
    Ok(MemoryFile {
        read_only: read_only.unwrap_or(false),
        frontmatter: frontmatter.into(),
        body: body.into(),
    })
}

fn parse_description(value: &str) -> Result<String, MemoryError> {
    let value = value.trim();
    let decoded = if value.starts_with('"') {
        serde_json::from_str::<String>(value).map_err(|_| MemoryError::Invalid)?
    } else {
        value.to_owned()
    };
    if decoded.trim().is_empty() {
        Err(MemoryError::Invalid)
    } else {
        Ok(decoded)
    }
}

fn parse_bool(value: &str) -> Result<bool, MemoryError> {
    match value.trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(MemoryError::Invalid),
    }
}

fn set_once<T>(slot: &mut Option<T>, value: T) -> Result<(), MemoryError> {
    if slot.is_some() {
        return Err(MemoryError::Invalid);
    }
    *slot = Some(value);
    Ok(())
}

fn write(
    path: RepositoryPath,
    file: &MemoryFile,
    body: &str,
) -> Result<Vec<MemFsMutation>, MemoryError> {
    Ok(vec![MemFsMutation::Write {
        path,
        contents: preserve_frontmatter(file, body)?,
    }])
}

fn preserve_frontmatter(file: &MemoryFile, body: &str) -> Result<MemoryFileContent, MemoryError> {
    let contents =
        MemoryFileContent::new(format!("---\n{}\n---\n{}", file.frontmatter, body).into_bytes())
            .map_err(|_| MemoryError::Invalid)?;
    validate_memory(&contents).map_err(|()| MemoryError::Invalid)?;
    Ok(contents)
}

fn render(
    description: &str,
    read_only: Option<bool>,
    body: &str,
) -> Result<MemoryFileContent, MemoryError> {
    if description.trim().is_empty() || description.contains(['\n', '\r', '\0']) {
        return Err(MemoryError::Invalid);
    }
    let protection = read_only.map_or(String::new(), |value| format!("read_only: {value}\n"));
    MemoryFileContent::new(
        format!(
            "---\ndescription: {}\n{}---\n{}",
            description.trim(),
            protection,
            body
        )
        .into_bytes(),
    )
    .map_err(|_| MemoryError::Invalid)
}

fn reject_symlink(root: &std::path::Path, path: &RepositoryPath) -> Result<(), MemoryError> {
    let mut current = root.to_owned();
    for component in path.as_path().components() {
        current.push(component);
        if let Ok(metadata) = std::fs::symlink_metadata(&current)
            && metadata.file_type().is_symlink()
        {
            return Err(MemoryError::Invalid);
        }
    }
    Ok(())
}

fn memory_path(value: &str) -> Result<RepositoryPath, MemoryError> {
    let mut path = value.strip_prefix("memory/").unwrap_or(value).to_owned();
    if !std::path::Path::new(&path)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
    {
        path.push_str(".md");
    }
    patch::memory_path(&path).map_err(|()| MemoryError::Invalid)
}

fn commit_message(value: &str) -> Result<CommitMessage, MemoryError> {
    if value.trim() != value || value.chars().any(char::is_control) {
        return Err(MemoryError::Invalid);
    }
    CommitMessage::new(value.into()).map_err(|_| MemoryError::Invalid)
}

fn required<'a>(input: &'a Value, name: &str) -> Result<&'a str, MemoryError> {
    let value = input
        .get(name)
        .and_then(Value::as_str)
        .ok_or(MemoryError::Invalid)?;
    if value.is_empty() {
        Err(MemoryError::Invalid)
    } else {
        Ok(value)
    }
}

fn optional<'a>(input: &'a Value, name: &str) -> Option<&'a str> {
    input.get(name).and_then(Value::as_str)
}

#[cfg(test)]
mod frontmatter_tests {
    use super::*;

    fn content(value: &str) -> MemoryFileContent {
        MemoryFileContent::new(value.as_bytes().to_vec()).expect("content")
    }

    #[test]
    fn canonical_frontmatter_matrix() {
        for valid in [
            "---\ndescription: plain\n---\nbody",
            "---\ndescription: \"quoted\\nvalue\"\nread_only: true\n---\nbody",
            "---\ndescription: plain\nread_only: false\nlimit: 10\n---\nbody",
        ] {
            assert!(validate_memory(&content(valid)).is_ok(), "valid {valid:?}");
        }
        for invalid in [
            "body",
            "---\ndescription:\n---\nbody",
            "---\ndescription: one\ndescription: two\n---\nbody",
            "---\ndescription: one\nread_only: yes\n---\nbody",
            "---\ndescription: one\nread_only: true\nread_only: false\n---\nbody",
            "---\ndescription: one\nunknown: value\n---\nbody",
            "---\ndescription: \"bad\\q\"\n---\nbody",
        ] {
            assert!(
                validate_memory(&content(invalid)).is_err(),
                "invalid {invalid:?}"
            );
        }
    }
}
