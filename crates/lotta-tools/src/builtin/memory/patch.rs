use super::patch_parse::{Hunk, Operation, Patch, parse_patch};
use lotta_domain::AgentId;
use lotta_runtime::boundary::{MemoryFileContent, RepositoryPath};
use lotta_runtime::ports::{MemFsMutation, MemFsPort};
use std::collections::{BTreeMap, BTreeSet};

const AGGREGATE_BYTES_MAX: usize = 64 * 1024 * 1024;
const SOURCE_LINES_MAX: usize = 100_000;

#[derive(Clone)]
struct Entry {
    original: Option<MemoryFileContent>,
    current: Option<MemoryFileContent>,
}

pub(super) fn parse(input: &str) -> Result<Patch<'_>, ()> {
    let patch = parse_patch(input, None).map_err(|_| ())?;
    confined_paths(&patch)?;
    Ok(patch)
}

pub(super) async fn mutations(
    port: &dyn MemFsPort,
    agent: &AgentId,
    patch: Patch<'_>,
) -> Result<Vec<MemFsMutation>, ()> {
    let paths = confined_paths(&patch)?;
    let mut overlay = load_overlay(port, agent, &paths).await?;
    for operation in patch.operations {
        apply_operation(&mut overlay, operation)?;
    }
    Ok(final_mutations(overlay))
}

fn confined_paths(patch: &Patch<'_>) -> Result<BTreeSet<RepositoryPath>, ()> {
    let mut result = BTreeSet::new();
    for operation in &patch.operations {
        match operation {
            Operation::Add { path, .. } | Operation::Delete { path } => {
                result.insert(memory_path(path)?);
            }
            Operation::Update {
                source,
                destination,
                ..
            } => {
                result.insert(memory_path(source)?);
                if let Some(target) = destination {
                    result.insert(memory_path(target)?);
                }
            }
        }
    }
    Ok(result)
}

async fn load_overlay(
    port: &dyn MemFsPort,
    agent: &AgentId,
    paths: &BTreeSet<RepositoryPath>,
) -> Result<BTreeMap<RepositoryPath, Entry>, ()> {
    let mut overlay = BTreeMap::new();
    let mut aggregate = 0usize;
    for path in paths {
        let value = match port.read(agent, path).await {
            Ok(value) => Some(value),
            Err(lotta_runtime::RuntimeError::NotFound { .. }) => None,
            Err(_) => return Err(()),
        };
        if let Some(contents) = value.as_ref() {
            aggregate = bounded_add(aggregate, contents.as_slice().len())?;
        }
        overlay.insert(
            path.clone(),
            Entry {
                original: value.clone(),
                current: value,
            },
        );
    }
    Ok(overlay)
}

fn apply_operation(
    overlay: &mut BTreeMap<RepositoryPath, Entry>,
    operation: Operation<'_>,
) -> Result<(), ()> {
    match operation {
        Operation::Add { path, lines } => apply_add(overlay, path, &lines),
        Operation::Delete { path } => apply_delete(overlay, path),
        Operation::Update {
            source,
            destination,
            hunks,
        } => apply_update(overlay, source, destination, &hunks),
    }
}

fn apply_add(
    overlay: &mut BTreeMap<RepositoryPath, Entry>,
    path: &str,
    lines: &[&str],
) -> Result<(), ()> {
    let path = memory_path(path)?;
    let entry = overlay.get_mut(&path).ok_or(())?;
    if entry.current.is_some() {
        return Err(());
    }
    entry.current = Some(render_added(lines)?);
    Ok(())
}

fn apply_delete(overlay: &mut BTreeMap<RepositoryPath, Entry>, path: &str) -> Result<(), ()> {
    let path = memory_path(path)?;
    let entry = overlay.get_mut(&path).ok_or(())?;
    let contents = entry.current.as_ref().ok_or(())?;
    ensure_editable(contents)?;
    entry.current = None;
    Ok(())
}

fn apply_update(
    overlay: &mut BTreeMap<RepositoryPath, Entry>,
    source: &str,
    destination: Option<&str>,
    hunks: &[Hunk<'_>],
) -> Result<(), ()> {
    let source = memory_path(source)?;
    let original = overlay
        .get(&source)
        .and_then(|entry| entry.current.clone())
        .ok_or(())?;
    ensure_editable(&original)?;
    let next = derive(&original, hunks)?;
    super::operations::validate_memory(&next)?;
    overlay.get_mut(&source).ok_or(())?.current = None;
    if let Some(destination) = destination {
        let destination = memory_path(destination)?;
        let target = overlay.get_mut(&destination).ok_or(())?;
        if destination != source && target.current.is_some() {
            return Err(());
        }
        target.current = Some(next);
    } else {
        overlay.get_mut(&source).ok_or(())?.current = Some(next);
    }
    Ok(())
}

fn final_mutations(overlay: BTreeMap<RepositoryPath, Entry>) -> Vec<MemFsMutation> {
    let mut result = Vec::new();
    for (path, entry) in overlay {
        if entry.original == entry.current {
            continue;
        }
        match entry.current {
            Some(contents) => result.push(MemFsMutation::Write { path, contents }),
            None => result.push(MemFsMutation::Delete { path }),
        }
    }
    result
}

fn derive(original: &MemoryFileContent, hunks: &[Hunk<'_>]) -> Result<MemoryFileContent, ()> {
    let text = std::str::from_utf8(original.as_slice()).map_err(|_| ())?;
    let final_newline = text.ends_with('\n');
    let source = text.strip_suffix('\n').unwrap_or(text);
    let mut lines: Vec<String> = if text.is_empty() {
        Vec::new()
    } else {
        source.split('\n').map(str::to_owned).collect()
    };
    if lines.len() > SOURCE_LINES_MAX {
        return Err(());
    }
    let mut cursor = 0usize;
    for hunk in hunks {
        if let Some(header) = hunk.header {
            cursor = seek_unique(&lines, &[header], cursor, false)?
                .checked_add(1)
                .ok_or(())?;
        }
        let at = if hunk.old.is_empty() {
            if hunk.eof {
                lines.len()
            } else {
                cursor.min(lines.len())
            }
        } else {
            seek_unique(&lines, &hunk.old, cursor, hunk.eof)?
        };
        let end = at.checked_add(hunk.old.len()).ok_or(())?;
        lines.splice(at..end, hunk.new.iter().map(|line| (*line).to_owned()));
        cursor = at.checked_add(hunk.new.len()).ok_or(())?;
    }
    render_lines(&lines, final_newline)
}

fn seek_unique(lines: &[String], pattern: &[&str], start: usize, eof: bool) -> Result<usize, ()> {
    let maximum = lines.len().checked_sub(pattern.len()).ok_or(())?;
    let first = if eof { maximum } else { start };
    for mode in 0..4 {
        let matches: Vec<usize> = (first..=maximum)
            .filter(|index| {
                pattern
                    .iter()
                    .enumerate()
                    .all(|(offset, value)| equivalent(&lines[index + offset], value, mode))
            })
            .collect();
        match matches.as_slice() {
            [index] => return Ok(*index),
            [] => (),
            _ => return Err(()),
        }
    }
    Err(())
}

fn equivalent(left: &str, right: &str, mode: u8) -> bool {
    match mode {
        0 => left == right,
        1 => left.trim_end() == right.trim_end(),
        2 => left.trim() == right.trim(),
        _ => normalize(left) == normalize(right),
    }
}

fn normalize(value: &str) -> String {
    value
        .trim()
        .chars()
        .map(|character| match character {
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
            | '\u{2212}' => '-',
            '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}' => '\'',
            '\u{201c}' | '\u{201d}' | '\u{201e}' | '\u{201f}' => '"',
            '\u{00a0}' | '\u{2002}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}' => ' ',
            other => other,
        })
        .collect()
}

fn render_lines(lines: &[String], final_newline: bool) -> Result<MemoryFileContent, ()> {
    let mut value = lines.join("\n");
    if final_newline && !lines.is_empty() {
        value.push('\n');
    }
    bounded_content(value.into_bytes())
}

fn render_added(lines: &[&str]) -> Result<MemoryFileContent, ()> {
    let mut body = lines.join("\n");
    if !lines.is_empty() {
        body.push('\n');
    }
    let value = if body.starts_with("---\n") {
        body
    } else {
        format!("---\ndescription: Memory file\n---\n{body}")
    };
    let contents = bounded_content(value.into_bytes())?;
    super::operations::validate_memory(&contents)?;
    Ok(contents)
}

fn bounded_content(value: Vec<u8>) -> Result<MemoryFileContent, ()> {
    if value.len() > AGGREGATE_BYTES_MAX {
        return Err(());
    }
    MemoryFileContent::new(value).map_err(|_| ())
}

fn ensure_editable(contents: &MemoryFileContent) -> Result<(), ()> {
    let parsed = super::operations::validate_memory(contents)?;
    if parsed.read_only { Err(()) } else { Ok(()) }
}

fn bounded_add(total: usize, amount: usize) -> Result<usize, ()> {
    let value = total.checked_add(amount).ok_or(())?;
    if value > AGGREGATE_BYTES_MAX {
        Err(())
    } else {
        Ok(value)
    }
}

pub(super) fn memory_path(value: &str) -> Result<RepositoryPath, ()> {
    let value = value.strip_prefix("memory/").unwrap_or(value);
    let markdown = std::path::Path::new(value)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"));
    if value.is_empty() || value.contains(['\0', '\\']) || !markdown {
        return Err(());
    }
    RepositoryPath::new(value.into()).map_err(|_| ())
}
