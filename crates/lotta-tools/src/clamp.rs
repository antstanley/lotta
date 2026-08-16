//! Unicode-scalar tool-result clamps with durable overflow retention.

use lotta_runtime::ports::ToolOutputLimit;
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use crate::limits::{TOOL_RESULT_BYTES_MAX, TOOL_RESULT_MODEL_CHARS_MAX};

const SHELL_TASK_READ_RETAINED: usize = 30_000;
const GREP_SEARCH_RETAINED: usize = 10_000;
const OVERFLOW_CREATE_RETRIES_MAX: usize = 32;
const OVERFLOW_NAME_BYTES_MAX: usize = 64;

/// Fixed overflow/clamp infrastructure failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClampError {
    /// Full scrubbed output could not be durably written.
    OverflowWrite,
    /// A tiny definition limit could not carry the required pointer.
    PointerTooLong,
    /// Allocation failed while constructing the model result.
    Allocation,
}

/// Durable sink for a complete scrubbed overflow value.
pub trait OverflowWriter: Send + Sync {
    /// Writes all content and returns its bounded absolute path.
    ///
    /// # Errors
    /// Returns [`ClampError`] when content is too large or cannot be durably written.
    fn write(&self, tool_name: &str, content: &str) -> Result<String, ClampError>;
}

/// Confined create-new file overflow writer.
pub struct FileOverflowWriter {
    directory: PathBuf,
    counter: AtomicU64,
}

impl FileOverflowWriter {
    /// Accepts a caller-owned canonical absolute plain directory without symlinks.
    ///
    /// # Errors
    /// Returns [`ClampError::OverflowWrite`] unless the directory meets all confinement rules.
    pub fn new(directory: PathBuf) -> Result<Self, ClampError> {
        if !directory.is_absolute()
            || !directory.is_dir()
            || directory
                .symlink_metadata()
                .map_or(true, |meta| meta.file_type().is_symlink())
            || directory.canonicalize().ok().as_ref() != Some(&directory)
        {
            return Err(ClampError::OverflowWrite);
        }
        Ok(Self {
            directory,
            counter: AtomicU64::new(0),
        })
    }
}

impl OverflowWriter for FileOverflowWriter {
    fn write(&self, tool_name: &str, content: &str) -> Result<String, ClampError> {
        if content.len() > TOOL_RESULT_BYTES_MAX.value {
            return Err(ClampError::OverflowWrite);
        }
        for _ in 0..OVERFLOW_CREATE_RETRIES_MAX {
            let count = self
                .counter
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                    value.checked_add(1)
                })
                .map_err(|_| ClampError::OverflowWrite)?;
            let path = self.directory.join(overflow_name(tool_name, count));
            match create_file(&path, content) {
                Ok(()) => return bounded_path(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err(ClampError::OverflowWrite),
            }
        }
        Err(ClampError::OverflowWrite)
    }
}

/// Clamps scrubbed success text after safely writing its complete value.
///
/// The middle split and two footer lines match the pinned `truncateByChars` behavior. The only
/// adjustment is the hard 32k/definition backstop, which reduces retained original text when the
/// unclassified 32k target (or a smaller byte/character definition) cannot also carry notices.
///
/// # Errors
/// Returns [`ClampError`] when overflow writing fails or the complete notice cannot fit.
pub fn clamp_text(
    internal_name: &str,
    text: &str,
    limit: ToolOutputLimit,
    writer: &dyn OverflowWriter,
) -> Result<String, ClampError> {
    let desired = family_limit(internal_name).min(limit.model_chars_max());
    let bytes_max = limit.bytes_max().min(TOOL_RESULT_BYTES_MAX.value);
    let chars_max = limit
        .model_chars_max()
        .min(TOOL_RESULT_MODEL_CHARS_MAX.value);
    if text.chars().count() <= desired && text.len() <= bytes_max {
        return Ok(text.to_owned());
    }
    let path = writer.write(internal_name, text)?;
    build_truncated(text, desired, chars_max, bytes_max, &path)
}

fn build_truncated(
    text: &str,
    desired: usize,
    chars_max: usize,
    bytes_max: usize,
    path: &str,
) -> Result<String, ClampError> {
    let total = text.chars().count();
    let mut retained = desired.min(total);
    loop {
        let omitted = total.checked_sub(retained).ok_or(ClampError::Allocation)?;
        let middle = format!("\n... [{} characters omitted] ...\n", comma(omitted)?);
        let footer = format!(
            "[Output truncated: showing {} of {} characters.]\n[Full output written to: {path}]",
            comma(retained)?,
            comma(total)?
        );
        let notice_chars = middle
            .chars()
            .count()
            .checked_add(footer.chars().count())
            .ok_or(ClampError::Allocation)?;
        let notice_bytes = middle
            .len()
            .checked_add(footer.len())
            .ok_or(ClampError::Allocation)?;
        if notice_chars > chars_max || notice_bytes > bytes_max {
            return Err(ClampError::PointerTooLong);
        }
        if retained <= chars_max - notice_chars
            && retained_bytes(text, retained) <= bytes_max - notice_bytes
        {
            return assemble(text, retained, &middle, &footer);
        }
        retained = retained.checked_sub(1).ok_or(ClampError::PointerTooLong)?;
    }
}

fn assemble(text: &str, retained: usize, middle: &str, footer: &str) -> Result<String, ClampError> {
    let head_chars = retained / 2;
    let tail_chars = retained - head_chars;
    let head_end = byte_after_chars(text, head_chars);
    let tail_start = byte_before_tail(text, tail_chars);
    let capacity = head_end
        .checked_add(text.len() - tail_start)
        .and_then(|value| value.checked_add(middle.len()))
        .and_then(|value| value.checked_add(footer.len()))
        .ok_or(ClampError::Allocation)?;
    let mut result = String::new();
    result
        .try_reserve(capacity)
        .map_err(|_| ClampError::Allocation)?;
    result.push_str(&text[..head_end]);
    result.push_str(middle);
    result.push_str(&text[tail_start..]);
    result.push_str(footer);
    Ok(result)
}

fn retained_bytes(text: &str, retained: usize) -> usize {
    let head = byte_after_chars(text, retained / 2);
    let tail = byte_before_tail(text, retained - retained / 2);
    head + text.len() - tail
}

fn byte_after_chars(text: &str, count: usize) -> usize {
    text.char_indices()
        .nth(count)
        .map_or(text.len(), |(index, _)| index)
}

fn byte_before_tail(text: &str, count: usize) -> usize {
    if count == 0 {
        text.len()
    } else {
        text.char_indices()
            .rev()
            .nth(count - 1)
            .map_or(0, |(index, _)| index)
    }
}

fn comma(value: usize) -> Result<String, ClampError> {
    let plain = value.to_string();
    let separators = if plain.is_empty() {
        0
    } else {
        (plain.len() - 1) / 3
    };
    let mut output = String::new();
    output
        .try_reserve(plain.len() + separators)
        .map_err(|_| ClampError::Allocation)?;
    for (index, character) in plain.chars().enumerate() {
        if index > 0 && (plain.len() - index).is_multiple_of(3) {
            output.push(',');
        }
        output.push(character);
    }
    Ok(output)
}

fn family_limit(name: &str) -> usize {
    match name {
        "Bash" | "exec_command" | "write_stdin" | "run_shell_command" | "Monitor" | "Task"
        | "TaskOutput" | "TaskCreate" | "TaskGet" | "TaskList" | "TaskUpdate" | "TaskStop"
        | "Read" | "read_file_gemini" | "read_file" | "read_many_files" => SHELL_TASK_READ_RETAINED,
        "Grep" | "grep" | "search_file_content" | "SearchFileContent" => GREP_SEARCH_RETAINED,
        _ => TOOL_RESULT_MODEL_CHARS_MAX.value,
    }
}

fn overflow_name(tool_name: &str, counter: u64) -> String {
    let safe: String = tool_name
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || *character == '_')
        .take(OVERFLOW_NAME_BYTES_MAX)
        .collect();
    format!(
        "{}-{}-{counter}.txt",
        if safe.is_empty() { "tool" } else { &safe },
        std::process::id()
    )
}

fn create_file(path: &Path, content: &str) -> std::io::Result<()> {
    let mut file: File = OpenOptions::new().write(true).create_new(true).open(path)?;
    let result = file
        .write_all(content.as_bytes())
        .and_then(|()| file.sync_all());
    drop(file);
    if result.is_err() {
        let _ignored = std::fs::remove_file(path);
    }
    result
}

fn bounded_path(path: PathBuf) -> Result<String, ClampError> {
    let value = path
        .into_os_string()
        .into_string()
        .map_err(|_| ClampError::OverflowWrite)?;
    if value.len() > 4_096 {
        Err(ClampError::OverflowWrite)
    } else {
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, sync::Mutex};

    struct MemoryWriter(Mutex<Option<String>>);
    impl OverflowWriter for MemoryWriter {
        fn write(&self, _: &str, content: &str) -> Result<String, ClampError> {
            *self.0.lock().map_err(|_| ClampError::OverflowWrite)? = Some(content.to_owned());
            Ok("/tmp/full.txt".into())
        }
    }
    struct ErrorWriter;
    impl OverflowWriter for ErrorWriter {
        fn write(&self, _: &str, _: &str) -> Result<String, ClampError> {
            Err(ClampError::OverflowWrite)
        }
    }
    fn limit() -> ToolOutputLimit {
        ToolOutputLimit::new(1024 * 1024, 32_000).unwrap()
    }
    fn assert_family(name: &str, retained: usize) {
        let writer = MemoryWriter(Mutex::new(None));
        let full = format!(
            "{}{}",
            "a".repeat(retained / 2),
            "z".repeat(retained / 2 + 2_000)
        );
        let result = clamp_text(name, &full, limit(), &writer).unwrap();
        let half = retained / 2;
        assert!(result.starts_with(&"a".repeat(half)));
        assert!(result.contains(&format!(
            "\n{}[Output truncated",
            "z".repeat(retained - half)
        )));
        assert!(result.contains(&format!("showing {} of", comma(retained).unwrap())));
        assert!(result.contains("[Full output written to: /tmp/full.txt]"));
        assert!(result.len() <= limit().bytes_max());
        assert!(result.chars().count() <= limit().model_chars_max());
        assert_eq!(writer.0.lock().unwrap().as_deref(), Some(full.as_str()));
    }
    #[test]
    fn shell_task_read_family() {
        for name in ["Bash", "Task", "Read"] {
            assert_family(name, 30_000);
        }
    }
    #[test]
    fn grep_family() {
        assert_family("search_file_content", 10_000);
    }
    #[test]
    fn unclassified_hard_backstop_reduces_original_only() {
        let writer = MemoryWriter(Mutex::new(None));
        let full = format!("{}{}", "a".repeat(32_000), "z".repeat(2_000));
        let result = clamp_text("other", &full, limit(), &writer).unwrap();
        assert!(result.chars().count() <= 32_000);
        assert!(!result.contains("showing 32,000 of"));
        assert!(result.contains("characters.]\n[Full output written to:"));
    }
    #[test]
    fn unicode_and_small_definition_obey_both_bounds() {
        let writer = MemoryWriter(Mutex::new(None));
        let small = ToolOutputLimit::new(400, 250).unwrap();
        let result = clamp_text("Read", &"é".repeat(500), small, &writer).unwrap();
        assert!(result.len() <= 400);
        assert!(result.chars().count() <= 250);
    }
    #[test]
    fn writer_error_and_tiny_pointer_are_exact() {
        assert_eq!(
            clamp_text("Read", &"x".repeat(30_001), limit(), &ErrorWriter),
            Err(ClampError::OverflowWrite)
        );
        let writer = MemoryWriter(Mutex::new(None));
        let tiny = ToolOutputLimit::new(10, 10).unwrap();
        assert_eq!(
            clamp_text("Read", &"x".repeat(100), tiny, &writer),
            Err(ClampError::PointerTooLong)
        );
        assert_eq!(
            writer.0.lock().unwrap().as_deref(),
            Some("x".repeat(100).as_str())
        );
    }
    #[test]
    fn file_writer_confinement_size_and_collisions() {
        assert!(FileOverflowWriter::new(PathBuf::from("relative")).is_err());
        let base = std::env::temp_dir().join(format!("lotta-clamp-{}", std::process::id()));
        let _ignored = fs::remove_dir_all(&base);
        fs::create_dir(&base).unwrap();
        let root = base.canonicalize().unwrap();
        assert!(FileOverflowWriter::new(root.join("missing")).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&root, root.join("link")).unwrap();
            assert!(FileOverflowWriter::new(root.join("link")).is_err());
        }
        let writer = FileOverflowWriter::new(root.clone()).unwrap();
        assert_eq!(
            writer.write("Read", &"x".repeat(TOOL_RESULT_BYTES_MAX.value + 1)),
            Err(ClampError::OverflowWrite)
        );
        let symlink_count = usize::from(cfg!(unix));
        let ordinary_count = fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .count();
        assert_eq!(ordinary_count, 0);
        assert_eq!(
            fs::read_dir(&root).unwrap().filter_map(Result::ok).count(),
            symlink_count
        );
        for counter in 0..OVERFLOW_CREATE_RETRIES_MAX {
            fs::write(
                root.join(overflow_name("Read", counter as u64)),
                "collision",
            )
            .unwrap();
        }
        assert_eq!(
            writer.write("Read", "content"),
            Err(ClampError::OverflowWrite)
        );
        assert_eq!(
            fs::read_dir(&root).unwrap().filter_map(Result::ok).count(),
            OVERFLOW_CREATE_RETRIES_MAX + symlink_count
        );
        fs::remove_dir_all(root).unwrap();
    }
}
