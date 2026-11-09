//! Server-sent events as the model APIs stream them: lines ending in `\n`, `\r\n` or `\r` (as the Anthropic SDK's
//! LineDecoder split them, a chunk boundary anywhere), then events of `event:` and `data:` fields up to a blank line
//! (as its SSEDecoder joined them).

/// Lines out of a byte stream; each line is decoded on its own, so a character split between chunks is whole.
#[derive(Debug, Default)]
pub struct Lines {
    buffer: Vec<u8>,
}

impl Lines {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(chunk);
        let mut lines = Vec::new();
        let mut start = 0;
        let mut at = 0;
        while at < self.buffer.len() {
            match self.buffer[at] {
                b'\n' => {
                    lines.push(decode(&self.buffer[start..at]));
                    at += 1;
                    start = at;
                }
                b'\r' => {
                    // A `\n` may follow in the next chunk: wait for it.
                    if at + 1 == self.buffer.len() {
                        break;
                    }
                    lines.push(decode(&self.buffer[start..at]));
                    at += if self.buffer[at + 1] == b'\n' { 2 } else { 1 };
                    start = at;
                }
                _ => at += 1,
            }
        }
        self.buffer.drain(..start);
        lines
    }

    /// What is left when the stream ends without a line break.
    pub fn finish(&mut self) -> Option<String> {
        if self.buffer.is_empty() {
            return None;
        }
        let mut rest = std::mem::take(&mut self.buffer);
        if rest.last() == Some(&b'\r') {
            rest.pop();
        }
        Some(decode(&rest))
    }
}

fn decode(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// None when the event named none.
    pub event: Option<String>,
    pub data: String,
}

/// Events out of lines.
#[derive(Debug, Default)]
pub struct SseDecoder {
    event: Option<String>,
    data: Vec<String>,
}

impl SseDecoder {
    /// The event a blank line completes; comments and other fields are skipped.
    pub fn line(&mut self, line: &str) -> Option<SseEvent> {
        if line.is_empty() {
            if self.event.as_deref().is_none_or(str::is_empty) && self.data.is_empty() {
                return None;
            }
            return Some(SseEvent { event: self.event.take(), data: std::mem::take(&mut self.data).join("\n") });
        }
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => self.event = Some(value.to_string()),
            "data" => self.data.push(value.to_string()),
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_lines_at_any_ending_across_chunks() {
        let mut lines = Lines::default();
        assert_eq!(lines.push(b"one\r"), Vec::<String>::new(), "a \\r waits for a possible \\n");
        assert_eq!(lines.push(b"\ntwo\nthr"), vec!["one", "two"]);
        assert_eq!(lines.push(b"ee\rfour\r\r"), vec!["three", "four"]);
        assert_eq!(lines.push("\nпр".as_bytes()), vec![""]);
        let word = "ивет".as_bytes();
        assert_eq!(lines.push(&word[..1]), Vec::<String>::new());
        assert_eq!(lines.push(&word[1..]), Vec::<String>::new());
        assert_eq!(lines.finish().as_deref(), Some("привет"), "a letter split between chunks is whole");
        assert_eq!(lines.finish(), None);
    }

    #[test]
    fn joins_fields_into_events_up_to_a_blank_line() {
        let mut decoder = SseDecoder::default();
        let mut events = Vec::new();
        for line in
            [": comment", "event: message_start", "data: {\"a\":", "data:1}", "id: 7", "", "", "data: [DONE]", ""]
        {
            events.extend(decoder.line(line));
        }
        assert_eq!(
            events,
            vec![
                SseEvent { event: Some("message_start".into()), data: "{\"a\":\n1}".into() },
                SseEvent { event: None, data: "[DONE]".into() },
            ]
        );
    }
}
