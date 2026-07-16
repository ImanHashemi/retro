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
    let marker_sub = retro_marker.rsplit_once(' ').map(|(_, s)| s).unwrap_or("");
    for group in arr.iter_mut() {
        if let Some(inner) = group.get_mut("hooks").and_then(|h| h.as_array_mut()) {
            for hook in inner.iter_mut() {
                let is_retro = hook
                    .get("command")
                    .and_then(|c| c.as_str())
                    .map(|c| is_retro_command(c, marker_sub))
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

/// Word-boundary identity for retro-owned hook commands: the program must BE
/// retro (bare or path-suffixed), not merely contain the word — mirrors
/// ensure_hook's never-hijack rule so install and uninstall agree on what
/// counts as ours (`my-retro observe` is foreign to both).
fn is_retro_command(command: &str, subcommand: &str) -> bool {
    let mut parts = command.split_whitespace();
    let prog = parts.next().unwrap_or("");
    let sub = parts.next().unwrap_or("");
    (prog == "retro" || prog.ends_with("/retro")) && sub == subcommand
}

/// Remove retro-owned hooks for `subcommand` from the event (word-boundary
/// identity — never third-party commands that merely contain "retro").
pub fn remove_retro_hook(settings: &mut Value, event: &str, subcommand: &str) -> bool {
    remove_hooks_where(settings, event, |c| is_retro_command(c, subcommand))
}

/// Remove hooks whose command contains `needle` — for retro-owned helper
/// scripts with distinctive filenames (e.g. `retro-briefing.sh`).
pub fn remove_hooks_containing(settings: &mut Value, event: &str, needle: &str) -> bool {
    remove_hooks_where(settings, event, |c| c.contains(needle))
}

/// Shared removal core. Emptied groups (and an emptied event key) are
/// dropped so the settings stay tidy. Returns true if anything was removed.
fn remove_hooks_where<F: Fn(&str) -> bool>(settings: &mut Value, event: &str, matches: F) -> bool {
    let Some(groups) = settings
        .get_mut("hooks")
        .and_then(|h| h.get_mut(event))
        .and_then(|e| e.as_array_mut())
    else {
        return false;
    };
    let mut removed = false;
    for group in groups.iter_mut() {
        if let Some(hooks) = group.get_mut("hooks").and_then(|h| h.as_array_mut()) {
            let before = hooks.len();
            hooks.retain(|h| {
                !h.get("command")
                    .and_then(|c| c.as_str())
                    .is_some_and(&matches)
            });
            removed |= hooks.len() != before;
        }
    }
    groups.retain(|g| {
        g.get("hooks")
            .and_then(|h| h.as_array())
            .is_none_or(|h| !h.is_empty())
    });
    if groups.is_empty() {
        if let Some(hooks) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) {
            hooks.remove(event);
        }
    }
    removed
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

    #[test]
    fn does_not_hijack_similarly_named_commands() {
        let existing = ensure_hook(
            json!({"hooks": {"SessionEnd": [
                {"matcher": "", "hooks": [{"type": "command", "command": "my-retro observe"}]}
            ]}}),
            "SessionEnd",
            "/bin/retro observe",
        )
        .unwrap();
        let arr = existing["hooks"]["SessionEnd"].as_array().unwrap();
        assert_eq!(arr.len(), 2, "foreign command preserved, retro appended");
        assert_eq!(arr[0]["hooks"][0]["command"], "my-retro observe");
    }

    #[test]
    fn remove_hook_strips_matching_entries_and_leaves_others() {
        let settings = ensure_hook(json!({}), "SessionEnd", "/some/retro observe").unwrap();
        let mut settings = ensure_hook(
            settings,
            "SessionEnd",
            "~/.masko-desktop/hooks/hook-sender.sh",
        )
        .unwrap();
        // a foreign command that merely CONTAINS "retro observe" must survive —
        // same never-hijack identity ensure_hook uses
        settings["hooks"]["SessionEnd"]
            .as_array_mut()
            .unwrap()
            .push(json!({"matcher": "", "hooks": [
                {"type": "command", "command": "~/scripts/my-retro observe.sh"},
                {"type": "command", "command": "/opt/retro observe"}
            ]}));

        assert!(remove_retro_hook(&mut settings, "SessionEnd", "observe"));
        let rendered = settings.to_string();
        assert!(!rendered.contains("/some/retro observe"));
        assert!(!rendered.contains("/opt/retro observe"), "path-suffixed retro removed");
        assert!(rendered.contains("hook-sender.sh"), "unrelated hooks preserved");
        assert!(
            rendered.contains("my-retro observe.sh"),
            "partial retain within a group: foreign near-name preserved"
        );
        assert!(
            !remove_retro_hook(&mut settings, "SessionEnd", "observe"),
            "idempotent"
        );

        // removing the last retro-owned event empties and drops the key
        let mut only_retro = ensure_hook(json!({}), "SessionStart", "retro brief").unwrap();
        assert!(remove_retro_hook(&mut only_retro, "SessionStart", "brief"));
        assert!(
            only_retro["hooks"].get("SessionStart").is_none(),
            "emptied event key dropped"
        );

        // contains-matching for retro-owned helper scripts
        let mut with_script = json!({"hooks": {"SessionStart": [
            {"matcher": "", "hooks": [{"type": "command", "command": "bash .claude/hooks/retro-briefing.sh"}]}
        ]}});
        assert!(remove_hooks_containing(&mut with_script, "SessionStart", "retro-briefing.sh"));
        assert!(with_script["hooks"].get("SessionStart").is_none());
    }
}
