//! Incremental, bounded Server-Sent Events parsing.

use lotta_runtime::bounds::PROVIDER_RESPONSE_EVENT_BYTES_MAX;

const SSE_LINE_BYTES_MAX: usize = PROVIDER_RESPONSE_EVENT_BYTES_MAX.value + "data: \r\n\r\n".len();
const SSE_EVENT_BYTES_MAX: usize = PROVIDER_RESPONSE_EVENT_BYTES_MAX.value;

/// One decoded SSE record.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SseEvent {
    /// Optional event name.
    pub event: Option<String>,
    /// Joined `data` fields, separated by LF.
    pub data: String,
    /// Optional last event identifier.
    pub id: Option<String>,
    /// Optional reconnection delay in milliseconds.
    pub retry: Option<u64>,
}

/// Bounded incremental SSE protocol failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SseError {
    /// Input was not valid UTF-8.
    #[error("invalid UTF-8 in SSE stream")]
    InvalidUtf8,
    /// A line or event exceeded its canonical response-event bound.
    #[error("SSE byte limit exceeded")]
    Limit,
}

/// Stateful SSE parser supporting arbitrary transport and UTF-8 boundaries.
#[derive(Debug, Default)]
pub struct SseParser {
    pending: Vec<u8>,
    event: SseEvent,
    data_seen: bool,
    first_line: bool,
}

impl SseParser {
    /// Creates an empty parser.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            pending: Vec::new(),
            event: SseEvent {
                event: None,
                data: String::new(),
                id: None,
                retry: None,
            },
            data_seen: false,
            first_line: true,
        }
    }

    /// Pushes one arbitrary byte chunk and returns complete events.
    ///
    /// # Errors
    /// Returns an error for malformed UTF-8 or an exceeded event bound.
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<SseEvent>, SseError> {
        let next = self
            .pending
            .len()
            .checked_add(chunk.len())
            .ok_or(SseError::Limit)?;
        if next > SSE_LINE_BYTES_MAX && !chunk.contains(&b'\n') {
            return Err(SseError::Limit);
        }
        self.pending.extend_from_slice(chunk);
        self.drain_lines()
    }

    fn drain_lines(&mut self) -> Result<Vec<SseEvent>, SseError> {
        let mut out = Vec::new();
        while let Some(position) = self.pending.iter().position(|byte| *byte == b'\n') {
            let mut line = self.pending.drain(..=position).collect::<Vec<_>>();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.line(&line, &mut out)?;
        }
        Ok(out)
    }

    fn line(&mut self, bytes: &[u8], out: &mut Vec<SseEvent>) -> Result<(), SseError> {
        let mut line = std::str::from_utf8(bytes).map_err(|_| SseError::InvalidUtf8)?;
        if self.first_line {
            line = line.strip_prefix('\u{feff}').unwrap_or(line);
            self.first_line = false;
        }
        if line.is_empty() {
            self.dispatch(out);
            return Ok(());
        }
        if line.starts_with(':') {
            return Ok(());
        }
        let (field, value) = line.split_once(':').map_or((line, ""), |(field, value)| {
            (field, value.strip_prefix(' ').unwrap_or(value))
        });
        self.field(field, value)
    }

    fn field(&mut self, field: &str, value: &str) -> Result<(), SseError> {
        match field {
            "data" => {
                let added = value.len() + usize::from(self.data_seen);
                if self
                    .event
                    .data
                    .len()
                    .checked_add(added)
                    .ok_or(SseError::Limit)?
                    > SSE_EVENT_BYTES_MAX
                {
                    return Err(SseError::Limit);
                }
                if self.data_seen {
                    self.event.data.push('\n');
                }
                self.event.data.push_str(value);
                self.data_seen = true;
            }
            "event" => self.event.event = Some(value.to_owned()),
            "id" if !value.contains('\0') => self.event.id = Some(value.to_owned()),
            "retry" => self.event.retry = value.parse().ok(),
            _ => {}
        }
        Ok(())
    }

    fn dispatch(&mut self, out: &mut Vec<SseEvent>) {
        if self.data_seen {
            out.push(std::mem::take(&mut self.event));
            self.data_seen = false;
        } else {
            self.event.event = None;
            self.event.retry = None;
        }
    }

    /// Completes parsing and dispatches a final unterminated record at clean EOF.
    ///
    /// # Errors
    /// Returns an error for malformed UTF-8 or an exceeded event bound.
    pub fn finish(mut self) -> Result<Vec<SseEvent>, SseError> {
        let mut out = Vec::new();
        if !self.pending.is_empty() {
            let line = std::mem::take(&mut self.pending);
            self.line(&line, &mut out)?;
        }
        self.dispatch(&mut out);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::{SSE_EVENT_BYTES_MAX, SseError, SseParser};

    #[test]
    fn handles_bom_chunks_multiline_named_events_and_clean_eof() {
        let mut parser = SseParser::new();
        assert!(parser.push(&[0xef]).unwrap().is_empty());
        assert!(parser.push(&[0xbb, 0xbf, b'e', b'v']).unwrap().is_empty());
        assert!(
            parser
                .push(b"ent: error\r\ndata: one\r\n:")
                .unwrap()
                .is_empty()
        );
        assert!(parser.push(b"comment\r\ndata: two").unwrap().is_empty());
        let events = parser.finish().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.as_deref(), Some("error"));
        assert_eq!(events[0].data, "one\ntwo");
    }

    #[test]
    fn accepts_at_bound_and_rejects_above() {
        let mut accepted = SseParser::new();
        let input = format!("data: {}\n", "x".repeat(SSE_EVENT_BYTES_MAX));
        assert!(accepted.push(input.as_bytes()).unwrap().is_empty());
        assert_eq!(
            accepted.finish().unwrap()[0].data.len(),
            SSE_EVENT_BYTES_MAX
        );
        let mut rejected = SseParser::new();
        let input = format!("data: {}\n", "x".repeat(SSE_EVENT_BYTES_MAX + 1));
        assert_eq!(rejected.push(input.as_bytes()), Err(SseError::Limit));
    }

    #[test]
    fn resets_event_name_between_records() {
        let mut parser = SseParser::new();
        let events = parser
            .push(b"event: named\ndata: one\n\ndata: two\n\n")
            .unwrap();
        assert_eq!(events[0].event.as_deref(), Some("named"));
        assert_eq!(events[1].event, None);
    }
}
