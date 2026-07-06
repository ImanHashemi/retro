//! Claude Code hook stdin event (SessionEnd / SessionStart payload).

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct HookEvent {
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub transcript_path: String,
    #[serde(default)]
    pub cwd: String,
}

impl HookEvent {
    /// Parse leniently: unknown fields ignored; empty/invalid input yields None
    /// (hook entries must never hard-fail on payload drift).
    pub fn parse(input: &str) -> Option<HookEvent> {
        let event: HookEvent = serde_json::from_str(input).ok()?;
        if event.session_id.is_empty() || event.transcript_path.is_empty() {
            return None;
        }
        Some(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_event_and_ignores_unknown_fields() {
        let json = r#"{"session_id":"abc-123","transcript_path":"/tmp/t.jsonl","cwd":"/tmp/proj","hook_event_name":"SessionEnd","extra":42}"#;
        let e = HookEvent::parse(json).unwrap();
        assert_eq!(e.session_id, "abc-123");
        assert_eq!(e.transcript_path, "/tmp/t.jsonl");
        assert_eq!(e.cwd, "/tmp/proj");
    }

    #[test]
    fn rejects_empty_or_invalid_input() {
        assert!(HookEvent::parse("").is_none());
        assert!(HookEvent::parse("not json").is_none());
        assert!(HookEvent::parse(r#"{"cwd":"/x"}"#).is_none()); // missing required fields
    }
}
