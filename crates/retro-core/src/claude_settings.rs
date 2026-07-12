//! Non-destructive editing of Claude Code settings JSON (global
//! ~/.claude/settings.json): ensure retro's hook entries exist without
//! touching anything else in the file.

use serde_json::{json, Value};

use crate::errors::CoreError;

/// Ensure a `{event}` hook running `command` exists. Identity: an existing
/// entry whose command ends with the same `retro <subcommand>` suffix is
/// retro's and gets updated in place (binary paths change across installs);
/// anything else is preserved untouched.
pub fn ensure_hook(mut settings: Value, event: &str, command: &str) -> Result<Value, CoreError> {
    let suffix = command
        .rsplit_once('/')
        .map(|(_, tail)| format!("/{tail}"))
        .unwrap_or_else(|| command.to_string());
    let retro_marker = suffix
        .rsplit_once('/')
        .map(|(_, t)| t.to_string())
        .unwrap_or(suffix.clone()); // e.g. "retro observe"

    if !settings.is_object() {
        return Err(CoreError::Parse(
            "settings.json is not a JSON object".to_string(),
        ));
    }
    let hooks = settings
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert_with(|| json!({}));
    if !hooks.is_object() {
        return Err(CoreError::Parse(
            "settings.json 'hooks' is not an object".to_string(),
        ));
    }
    let event_arr = hooks
        .as_object_mut()
        .unwrap()
        .entry(event)
        .or_insert_with(|| json!([]));
    let Some(arr) = event_arr.as_array_mut() else {
        return Err(CoreError::Parse(format!("hooks.{event} is not an array")));
    };

    // Update an existing retro entry in place.
    for group in arr.iter_mut() {
        if let Some(inner) = group.get_mut("hooks").and_then(|h| h.as_array_mut()) {
            for hook in inner.iter_mut() {
                let is_retro = hook
                    .get("command")
                    .and_then(|c| c.as_str())
                    .map(|c| c.ends_with(&retro_marker))
                    .unwrap_or(false);
                if is_retro {
                    hook["command"] = json!(command);
                    return Ok(settings);
                }
            }
        }
    }
    arr.push(json!({
        "matcher": "",
        "hooks": [{"type": "command", "command": command}]
    }));
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_hook_to_empty_settings() {
        let out = ensure_hook(json!({}), "SessionEnd", "/usr/local/bin/retro observe").unwrap();
        let arr = out["hooks"]["SessionEnd"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(
            arr[0]["hooks"][0]["command"],
            "/usr/local/bin/retro observe"
        );
    }

    #[test]
    fn preserves_existing_unrelated_hooks_and_settings() {
        let existing = json!({
            "model": "opus",
            "hooks": {
                "SessionEnd": [
                    {"matcher": "", "hooks": [{"type": "command", "command": "other-tool cleanup"}]}
                ],
                "PostToolUse": [
                    {"matcher": "Bash", "hooks": [{"type": "command", "command": "audit.sh"}]}
                ]
            }
        });
        let out = ensure_hook(existing, "SessionEnd", "/bin/retro observe").unwrap();
        assert_eq!(out["model"], "opus");
        let se = out["hooks"]["SessionEnd"].as_array().unwrap();
        assert_eq!(se.len(), 2, "appended, not replaced");
        assert!(out["hooks"]["PostToolUse"].is_array());
    }

    #[test]
    fn is_idempotent_by_command_substring() {
        let once = ensure_hook(json!({}), "SessionStart", "/bin/retro brief").unwrap();
        let twice = ensure_hook(once.clone(), "SessionStart", "/bin/retro brief").unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn recognizes_retro_hook_even_if_binary_path_changed() {
        let old = ensure_hook(json!({}), "SessionEnd", "/old/path/retro observe").unwrap();
        let new = ensure_hook(old, "SessionEnd", "/new/path/retro observe").unwrap();
        let arr = new["hooks"]["SessionEnd"].as_array().unwrap();
        assert_eq!(arr.len(), 1, "updated in place, not duplicated");
        assert_eq!(arr[0]["hooks"][0]["command"], "/new/path/retro observe");
    }
}
