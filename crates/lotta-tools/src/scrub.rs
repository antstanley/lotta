//! Single-pass secret scrubbing for bounded tool outcomes.

use crate::limits::TOOL_RESULT_BYTES_MAX;

/// Fixed scrubbing failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScrubError {
    /// Scrubbed text exceeded the retained tool-result bound.
    ResultTooLarge,
    /// Allocation failed while producing scrubbed text.
    Allocation,
}

/// Replaces each nonempty secret value in the original input with its named marker.
///
/// At a shared start position the longest value wins; equal lengths choose the smallest name.
/// Replacement markers are never rescanned.
///
/// # Errors
/// Returns [`ScrubError`] when output would exceed the global result limit or allocation fails.
pub fn scrub_text(text: &str, secrets: &[(&str, &str)]) -> Result<String, ScrubError> {
    let mut output = String::new();
    output
        .try_reserve(text.len().min(TOOL_RESULT_BYTES_MAX.value))
        .map_err(|_| ScrubError::Allocation)?;
    let mut cursor = 0;
    while cursor < text.len() {
        let Some((start, name, value)) = next_match(text, cursor, secrets) else {
            append(&mut output, &text[cursor..])?;
            break;
        };
        append(&mut output, &text[cursor..start])?;
        append(&mut output, name)?;
        append(&mut output, "=<REDACTED>")?;
        cursor = start
            .checked_add(value.len())
            .ok_or(ScrubError::ResultTooLarge)?;
    }
    if text.is_empty() {
        return Ok(output);
    }
    Ok(output)
}

fn next_match<'a>(
    text: &'a str,
    cursor: usize,
    secrets: &'a [(&'a str, &'a str)],
) -> Option<(usize, &'a str, &'a str)> {
    let mut best: Option<(usize, &str, &str)> = None;
    for &(name, value) in secrets {
        if value.is_empty() {
            continue;
        }
        if let Some(relative) = text[cursor..].find(value) {
            let start = cursor + relative;
            if best.is_none_or(|(old, old_name, old_value)| {
                start < old
                    || (start == old
                        && (value.len() > old_value.len()
                            || (value.len() == old_value.len() && name < old_name)))
            }) {
                best = Some((start, name, value));
            }
        }
    }
    best
}

fn append(output: &mut String, value: &str) -> Result<(), ScrubError> {
    let next = output
        .len()
        .checked_add(value.len())
        .ok_or(ScrubError::ResultTooLarge)?;
    if next > TOOL_RESULT_BYTES_MAX.value {
        return Err(ScrubError::ResultTooLarge);
    }
    output
        .try_reserve(value.len())
        .map_err(|_| ScrubError::Allocation)?;
    output.push_str(value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_longest_and_unicode() {
        assert_eq!(
            scrub_text("éfoobar", &[("SHORT", "foo"), ("LONG", "foobar")]),
            Ok("éLONG=<REDACTED>".into())
        );
    }

    #[test]
    fn equal_length_same_start_uses_lexicographically_smallest_name() {
        assert_eq!(
            scrub_text("secret", &[("ZED", "secret"), ("ALPHA", "secret")]),
            Ok("ALPHA=<REDACTED>".into())
        );
    }

    #[test]
    fn markers_are_not_rescanned() {
        assert_eq!(
            scrub_text("secret RED", &[("A", "secret"), ("B", "RED")]),
            Ok("A=<REDACTED> B=<REDACTED>".into())
        );
    }

    #[test]
    fn no_secret_is_unchanged() {
        assert_eq!(scrub_text("plain", &[("A", "")]), Ok("plain".into()));
    }

    #[test]
    fn expansion_is_bounded() {
        let input = "x".repeat(TOOL_RESULT_BYTES_MAX.value);
        assert_eq!(
            scrub_text(&input, &[("LONG_NAME", "x")]),
            Err(ScrubError::ResultTooLarge)
        );
    }
}
