# Full CLAUDE.md Management — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add full CLAUDE.md management to retro — granular edits via the existing review queue + a new `retro curate` command for AI-powered full rewrites.

**Architecture:** Two-mode system gated by `[claude_md] full_management` config. Granular edits extend the analysis pipeline with `claude_md_edits` in the AI response. Full rewrites use an agentic Claude CLI call (unlimited turns, tool access) piped to a PR. See `docs/plans/2026-02-26-full-claude-md-management-design.md` for the full design.

**Tech Stack:** Rust, clap, rusqlite, serde_json, std::process::Command (Claude CLI), git/gh shell-outs.

---

### Task 1: Add `ClaudeMdConfig` to config

**Files:**
- Modify: `crates/retro-core/src/config.rs`

**Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block at the bottom of `config.rs`:

```rust
#[test]
fn test_claude_md_config_defaults() {
    let config = Config::default();
    assert!(!config.claude_md.full_management);
}

#[test]
fn test_claude_md_config_custom() {
    let toml_str = r#"
[claude_md]
full_management = true
"#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert!(config.claude_md.full_management);
}

#[test]
fn test_claude_md_config_absent() {
    // Config with no [claude_md] section should use defaults
    let toml_str = r#"
[analysis]
window_days = 14
"#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert!(!config.claude_md.full_management);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p retro-core test_claude_md_config`
Expected: FAIL — `claude_md` field doesn't exist on `Config`

**Step 3: Write minimal implementation**

Add the struct and default function in `config.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeMdConfig {
    #[serde(default)]
    pub full_management: bool,
}

fn default_claude_md() -> ClaudeMdConfig {
    ClaudeMdConfig {
        full_management: false,
    }
}
```

Add to the `Config` struct:

```rust
#[serde(default = "default_claude_md")]
pub claude_md: ClaudeMdConfig,
```

Add to `Config::default()`:

```rust
claude_md: default_claude_md(),
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p retro-core test_claude_md_config`
Expected: PASS (3 tests)

**Step 5: Commit**

```bash
git add crates/retro-core/src/config.rs
git commit -m "feat: add [claude_md] config section with full_management option"
```

---

### Task 2: Add `ClaudeMdEdit` model types

**Files:**
- Modify: `crates/retro-core/src/models.rs`

**Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `models.rs`:

```rust
#[test]
fn test_claude_md_edit_type_serde() {
    let edit = ClaudeMdEdit {
        edit_type: ClaudeMdEditType::Reword,
        original_text: "No async".to_string(),
        suggested_content: Some("Sync only — no tokio, no async".to_string()),
        target_section: None,
        reasoning: "Too terse".to_string(),
    };
    let json = serde_json::to_string(&edit).unwrap();
    let parsed: ClaudeMdEdit = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.edit_type, ClaudeMdEditType::Reword);
    assert_eq!(parsed.original_text, "No async");
    assert_eq!(parsed.suggested_content.unwrap(), "Sync only — no tokio, no async");
}

#[test]
fn test_claude_md_edit_type_display() {
    assert_eq!(ClaudeMdEditType::Add.to_string(), "add");
    assert_eq!(ClaudeMdEditType::Remove.to_string(), "remove");
    assert_eq!(ClaudeMdEditType::Reword.to_string(), "reword");
    assert_eq!(ClaudeMdEditType::Move.to_string(), "move");
}

#[test]
fn test_analysis_response_with_edits() {
    let json = r#"{
        "reasoning": "test",
        "patterns": [],
        "claude_md_edits": [
            {
                "edit_type": "remove",
                "original_text": "stale rule",
                "reasoning": "no longer relevant"
            }
        ]
    }"#;
    let resp: AnalysisResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.claude_md_edits.len(), 1);
    assert_eq!(resp.claude_md_edits[0].edit_type, ClaudeMdEditType::Remove);
}

#[test]
fn test_analysis_response_without_edits() {
    let json = r#"{"reasoning": "test", "patterns": []}"#;
    let resp: AnalysisResponse = serde_json::from_str(json).unwrap();
    assert!(resp.claude_md_edits.is_empty());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p retro-core test_claude_md_edit`
Expected: FAIL — types don't exist

**Step 3: Write minimal implementation**

Add to `models.rs` in the "Analysis types" section (after `AnalysisResponse`):

```rust
// ── CLAUDE.md edit types ──

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ClaudeMdEditType {
    Add,
    Remove,
    Reword,
    Move,
}

impl std::fmt::Display for ClaudeMdEditType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Add => write!(f, "add"),
            Self::Remove => write!(f, "remove"),
            Self::Reword => write!(f, "reword"),
            Self::Move => write!(f, "move"),
        }
    }
}

/// A proposed edit to existing CLAUDE.md content (when full_management = true).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeMdEdit {
    pub edit_type: ClaudeMdEditType,
    #[serde(default)]
    pub original_text: String,
    #[serde(default)]
    pub suggested_content: Option<String>,
    #[serde(default)]
    pub target_section: Option<String>,
    #[serde(default)]
    pub reasoning: String,
}
```

Extend `AnalysisResponse`:

```rust
pub struct AnalysisResponse {
    #[serde(default)]
    pub reasoning: String,
    pub patterns: Vec<PatternUpdate>,
    #[serde(default)]
    pub claude_md_edits: Vec<ClaudeMdEdit>,
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p retro-core test_claude_md_edit && cargo test -p retro-core test_analysis_response`
Expected: PASS (4 new tests + existing tests still pass)

**Step 5: Commit**

```bash
git add crates/retro-core/src/models.rs
git commit -m "feat: add ClaudeMdEdit types and extend AnalysisResponse"
```

---

### Task 3: Add delimiter dissolution to `claude_md.rs`

**Files:**
- Modify: `crates/retro-core/src/projection/claude_md.rs`

**Step 1: Write the failing test**

Add to tests in `claude_md.rs`:

```rust
#[test]
fn test_dissolve_managed_section() {
    let content = format!(
        "# My Project\n\nSome content.\n\n{}\n## Retro-Discovered Patterns\n\n- Rule A\n- Rule B\n\n{}\n\n## Footer\n",
        MANAGED_START, MANAGED_END
    );
    let result = dissolve_managed_section(&content);
    // Should keep rule content but strip markers and header
    assert!(!result.contains(MANAGED_START));
    assert!(!result.contains(MANAGED_END));
    assert!(!result.contains("## Retro-Discovered Patterns"));
    assert!(result.contains("- Rule A"));
    assert!(result.contains("- Rule B"));
    assert!(result.contains("# My Project"));
    assert!(result.contains("## Footer"));
}

#[test]
fn test_dissolve_no_managed_section() {
    let content = "# My Project\n\nNo managed section.\n";
    let result = dissolve_managed_section(content);
    assert_eq!(result, content);
}

#[test]
fn test_has_managed_section() {
    let with = format!("content\n{}\nrules\n{}\n", MANAGED_START, MANAGED_END);
    let without = "just content\n";
    assert!(has_managed_section(&with));
    assert!(!has_managed_section(without));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p retro-core test_dissolve && cargo test -p retro-core test_has_managed`
Expected: FAIL — functions don't exist

**Step 3: Write minimal implementation**

Add to `claude_md.rs`:

```rust
/// Check if content contains a managed section.
pub fn has_managed_section(content: &str) -> bool {
    content.contains(MANAGED_START) && content.contains(MANAGED_END)
}

/// Remove managed section delimiters and header, keeping rule content in place.
/// Used when transitioning to full_management mode.
pub fn dissolve_managed_section(content: &str) -> String {
    let Some((before, inner, after)) = split_managed(content) else {
        return content.to_string();
    };

    // Strip the "## Retro-Discovered Patterns" header from inner content,
    // keep the rule lines
    let cleaned_inner: String = inner
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            trimmed != "## Retro-Discovered Patterns"
        })
        .collect::<Vec<_>>()
        .join("\n");

    // Reassemble: before + cleaned rules + after
    let mut result = before;
    if !cleaned_inner.trim().is_empty() {
        result.push_str(&cleaned_inner);
    }
    result.push_str(&after);
    result
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p retro-core test_dissolve && cargo test -p retro-core test_has_managed`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/retro-core/src/projection/claude_md.rs
git commit -m "feat: add dissolve_managed_section and has_managed_section"
```

---

### Task 4: Add CLAUDE.md edit operations to `claude_md.rs`

**Files:**
- Modify: `crates/retro-core/src/projection/claude_md.rs`

**Step 1: Write the failing tests**

Add to tests in `claude_md.rs`:

```rust
use crate::models::{ClaudeMdEdit, ClaudeMdEditType};

#[test]
fn test_apply_edit_remove() {
    let content = "# Project\n\n- Use thiserror in lib crates\n- Stale rule to remove\n\n## More\n";
    let edit = ClaudeMdEdit {
        edit_type: ClaudeMdEditType::Remove,
        original_text: "- Stale rule to remove".to_string(),
        suggested_content: None,
        target_section: None,
        reasoning: "stale".to_string(),
    };
    let result = apply_edit(content, &edit);
    assert!(!result.contains("Stale rule to remove"));
    assert!(result.contains("Use thiserror"));
    assert!(result.contains("## More"));
}

#[test]
fn test_apply_edit_reword() {
    let content = "# Project\n\nNo async\n\n## More\n";
    let edit = ClaudeMdEdit {
        edit_type: ClaudeMdEditType::Reword,
        original_text: "No async".to_string(),
        suggested_content: Some("Sync only — no tokio, no async".to_string()),
        target_section: None,
        reasoning: "too terse".to_string(),
    };
    let result = apply_edit(content, &edit);
    assert!(!result.contains("\nNo async\n"));
    assert!(result.contains("Sync only — no tokio, no async"));
}

#[test]
fn test_apply_edit_add() {
    let content = "# Project\n\nExisting content.\n";
    let edit = ClaudeMdEdit {
        edit_type: ClaudeMdEditType::Add,
        original_text: String::new(),
        suggested_content: Some("- New rule to add".to_string()),
        target_section: None,
        reasoning: "new pattern".to_string(),
    };
    let result = apply_edit(content, &edit);
    assert!(result.contains("Existing content."));
    assert!(result.contains("- New rule to add"));
}

#[test]
fn test_apply_edits_batch() {
    let content = "# Project\n\nRule A\nRule B\nRule C\n";
    let edits = vec![
        ClaudeMdEdit {
            edit_type: ClaudeMdEditType::Remove,
            original_text: "Rule B".to_string(),
            suggested_content: None,
            target_section: None,
            reasoning: "stale".to_string(),
        },
        ClaudeMdEdit {
            edit_type: ClaudeMdEditType::Reword,
            original_text: "Rule A".to_string(),
            suggested_content: Some("Rule A (improved)".to_string()),
            target_section: None,
            reasoning: "clarity".to_string(),
        },
    ];
    let result = apply_edits(content, &edits);
    assert!(!result.contains("\nRule B\n"));
    assert!(result.contains("Rule A (improved)"));
    assert!(result.contains("Rule C"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p retro-core test_apply_edit`
Expected: FAIL — `apply_edit` and `apply_edits` don't exist

**Step 3: Write minimal implementation**

Add to `claude_md.rs`:

```rust
use crate::models::{ClaudeMdEdit, ClaudeMdEditType};

/// Apply a single edit to CLAUDE.md content.
pub fn apply_edit(content: &str, edit: &ClaudeMdEdit) -> String {
    match edit.edit_type {
        ClaudeMdEditType::Remove => {
            // Find and remove the original text line(s)
            content
                .lines()
                .filter(|line| line.trim() != edit.original_text.trim())
                .collect::<Vec<_>>()
                .join("\n")
                + if content.ends_with('\n') { "\n" } else { "" }
        }
        ClaudeMdEditType::Reword => {
            // Find original text and replace with suggested content
            if let Some(replacement) = &edit.suggested_content {
                content.replace(edit.original_text.trim(), replacement.trim())
            } else {
                content.to_string()
            }
        }
        ClaudeMdEditType::Add => {
            // Append suggested content at end
            let mut result = content.to_string();
            if let Some(new_content) = &edit.suggested_content {
                if !result.ends_with('\n') {
                    result.push('\n');
                }
                result.push_str(new_content);
                result.push('\n');
            }
            result
        }
        ClaudeMdEditType::Move => {
            // Remove from current location, append to target section
            let without = content
                .lines()
                .filter(|line| line.trim() != edit.original_text.trim())
                .collect::<Vec<_>>()
                .join("\n");

            if let (Some(section), Some(text)) = (&edit.target_section, &edit.suggested_content) {
                // Find target section header and insert after it
                let mut result = String::new();
                let mut inserted = false;
                for line in without.lines() {
                    result.push_str(line);
                    result.push('\n');
                    if !inserted && line.trim().starts_with('#') && line.contains(section) {
                        result.push_str(text);
                        result.push('\n');
                        inserted = true;
                    }
                }
                if !inserted {
                    // Fallback: append at end
                    result.push_str(text);
                    result.push('\n');
                }
                result
            } else {
                without + "\n"
            }
        }
    }
}

/// Apply a batch of edits to CLAUDE.md content, in order.
pub fn apply_edits(content: &str, edits: &[ClaudeMdEdit]) -> String {
    let mut result = content.to_string();
    for edit in edits {
        result = apply_edit(&result, edit);
    }
    result
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p retro-core test_apply_edit`
Expected: PASS (4 tests)

**Step 5: Commit**

```bash
git add crates/retro-core/src/projection/claude_md.rs
git commit -m "feat: add apply_edit and apply_edits for granular CLAUDE.md editing"
```

---

### Task 5: Extend `ANALYSIS_RESPONSE_SCHEMA` for `claude_md_edits`

**Files:**
- Modify: `crates/retro-core/src/analysis/mod.rs`

**Step 1: Write the failing test**

Add a test in `mod.rs` (or add to existing tests):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analysis_schema_includes_edits() {
        // When full_management is true, the schema should include claude_md_edits
        let schema = full_management_analysis_schema();
        assert!(schema.contains("claude_md_edits"));
        assert!(schema.contains("edit_type"));
        assert!(schema.contains("original_text"));
    }

    #[test]
    fn test_base_analysis_schema_no_edits() {
        // The base schema should NOT include claude_md_edits
        assert!(!ANALYSIS_RESPONSE_SCHEMA.contains("claude_md_edits"));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p retro-core test_analysis_schema`
Expected: FAIL — `full_management_analysis_schema` doesn't exist

**Step 3: Write minimal implementation**

Add a new constant/function in `analysis/mod.rs`:

```rust
/// Extended JSON schema that includes claude_md_edits for full_management mode.
pub fn full_management_analysis_schema() -> String {
    r#"{
  "type": "object",
  "properties": {
    "reasoning": {"type": "string"},
    "patterns": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "action": {"type": "string", "enum": ["new", "update"]},
          "pattern_type": {"type": "string", "enum": ["repetitive_instruction", "recurring_mistake", "workflow_pattern", "stale_context", "redundant_context"]},
          "description": {"type": "string"},
          "confidence": {"type": "number"},
          "source_sessions": {"type": "array", "items": {"type": "string"}},
          "related_files": {"type": "array", "items": {"type": "string"}},
          "suggested_content": {"type": "string"},
          "suggested_target": {"type": "string", "enum": ["skill", "claude_md", "global_agent", "db_only"]},
          "existing_id": {"type": "string"},
          "new_sessions": {"type": "array", "items": {"type": "string"}},
          "new_confidence": {"type": "number"}
        },
        "required": ["action"],
        "additionalProperties": false
      }
    },
    "claude_md_edits": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "edit_type": {"type": "string", "enum": ["add", "remove", "reword", "move"]},
          "original_text": {"type": "string"},
          "suggested_content": {"type": "string"},
          "target_section": {"type": "string"},
          "reasoning": {"type": "string"}
        },
        "required": ["edit_type", "reasoning"],
        "additionalProperties": false
      }
    }
  },
  "required": ["reasoning", "patterns"],
  "additionalProperties": false
}"#.to_string()
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p retro-core test_analysis_schema`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/retro-core/src/analysis/mod.rs
git commit -m "feat: add full_management_analysis_schema with claude_md_edits"
```

---

### Task 6: Extend analysis prompt for full CLAUDE.md examination

**Files:**
- Modify: `crates/retro-core/src/analysis/prompts.rs`

**Step 1: Write the failing test**

Add to tests in `prompts.rs` (check if test module exists, otherwise create one):

```rust
#[test]
fn test_build_analysis_prompt_full_management() {
    let sessions = vec![]; // empty is fine for prompt structure test
    let patterns = vec![];
    let context = "existing CLAUDE.md content here";
    let prompt = build_analysis_prompt(&sessions, &patterns, Some(context), true);
    assert!(prompt.contains("claude_md_edits"));
    assert!(prompt.contains("edit_type"));
    assert!(prompt.contains("reword"));
    assert!(prompt.contains("remove"));
}

#[test]
fn test_build_analysis_prompt_no_full_management() {
    let sessions = vec![];
    let patterns = vec![];
    let context = "existing CLAUDE.md content here";
    let prompt = build_analysis_prompt(&sessions, &patterns, Some(context), false);
    assert!(!prompt.contains("claude_md_edits"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p retro-core test_build_analysis_prompt_full`
Expected: FAIL — `build_analysis_prompt` doesn't accept a `full_management` parameter

**Step 3: Write minimal implementation**

Add a `full_management: bool` parameter to `build_analysis_prompt()`. When `true`, append an additional section to the prompt:

```rust
pub fn build_analysis_prompt(
    sessions: &[Session],
    existing_patterns: &[Pattern],
    context_summary: Option<&str>,
    full_management: bool,  // NEW parameter
) -> String {
    // ... existing prompt building ...

    if full_management {
        prompt.push_str(r#"

## CLAUDE.md Edits (full_management mode)

In addition to discovering patterns, examine the FULL CLAUDE.md content provided in the context.
Propose edits to improve existing content — not just new rules. Return these in a `claude_md_edits` array.

Each edit has:
- `edit_type`: "add" | "remove" | "reword" | "move"
- `original_text`: the exact text being edited (for remove/reword/move — must match the file)
- `suggested_content`: the replacement text (for add/reword/move)
- `target_section`: the section header to move content to (for move only)
- `reasoning`: why this edit improves the CLAUDE.md

Guidelines:
- **remove**: delete stale, redundant, or incorrect rules
- **reword**: improve clarity, accuracy, or conciseness of existing rules
- **move**: relocate content to a more logical section
- **add**: add new content (same as regular claude_md patterns, but use this for content that doesn't fit the pattern model)
- Be conservative — only propose edits you're confident improve the document
- Match `original_text` exactly as it appears in the file (for reliable find-and-replace)
- If no edits are needed, return an empty `claude_md_edits` array
"#);
    }

    // ... rest of prompt ...
}
```

Update all callers of `build_analysis_prompt` to pass `false` for the new parameter (preserving existing behavior). The only caller passing `true` will be in the analyze function when `config.claude_md.full_management` is set.

**Step 4: Run test to verify it passes**

Run: `cargo test -p retro-core test_build_analysis_prompt`
Expected: PASS (new + existing tests)

**Step 5: Run full test suite**

Run: `cargo test -p retro-core`
Expected: All tests pass (callers updated)

**Step 6: Commit**

```bash
git add crates/retro-core/src/analysis/prompts.rs
git commit -m "feat: extend analysis prompt for full CLAUDE.md editing"
```

---

### Task 7: Wire full_management through the analysis pipeline

**Files:**
- Modify: `crates/retro-core/src/analysis/mod.rs` (the `analyze()` function)
- Modify: `crates/retro-core/src/analysis/prompts.rs` (caller update)

**Step 1: Write the failing test**

This is an integration-level change. The key behavior: when `config.claude_md.full_management` is `true`, the `analyze()` function should:
1. Use `full_management_analysis_schema()` instead of `ANALYSIS_RESPONSE_SCHEMA`
2. Pass `full_management=true` to `build_analysis_prompt()`

Since `analyze()` depends on `AnalysisBackend`, test with the existing `MockBackend` pattern. Add a test that verifies the schema used:

```rust
// In analysis/mod.rs tests or a new integration test
#[test]
fn test_analyze_selects_schema_by_config() {
    // Verify that when full_management is true, the schema includes claude_md_edits
    let schema = full_management_analysis_schema();
    let parsed: serde_json::Value = serde_json::from_str(&schema).unwrap();
    let props = parsed["properties"].as_object().unwrap();
    assert!(props.contains_key("claude_md_edits"));
}
```

**Step 2: Run test to verify it passes (schema test)**

Run: `cargo test -p retro-core test_analyze_selects_schema`
Expected: PASS (this validates the schema is valid JSON)

**Step 3: Write implementation**

In `analyze()` function in `analysis/mod.rs`, around the prompt/schema selection:

```rust
// Choose schema based on full_management config
let schema = if config.claude_md.full_management {
    full_management_analysis_schema()
} else {
    ANALYSIS_RESPONSE_SCHEMA.to_string()
};

// Pass full_management flag to prompt builder
let prompt = prompts::build_analysis_prompt(
    &batch_sessions,
    &current_patterns,
    context_summary.as_deref(),
    config.claude_md.full_management,
);

// Use the chosen schema
let response = backend.execute(&prompt, Some(&schema))?;
```

**Step 4: Run full test suite**

Run: `cargo test -p retro-core`
Expected: All tests pass

**Step 5: Commit**

```bash
git add crates/retro-core/src/analysis/mod.rs crates/retro-core/src/analysis/prompts.rs
git commit -m "feat: wire full_management config through analysis pipeline"
```

---

### Task 8: Handle `claude_md_edits` in the apply plan

**Files:**
- Modify: `crates/retro-core/src/projection/mod.rs`
- Modify: `crates/retro-core/src/models.rs` (extend `ApplyAction` if needed)

**Step 1: Design the approach**

The `claude_md_edits` from analysis need to flow through to the review queue. They should become `ApplyAction` items with the edit stored as JSON in the `content` field. The `target_type` is `ClaudeMd` and the `track` is `Shared`.

We need a way to distinguish between "add a new rule" (existing behavior) and "edit existing content" (new behavior). Store the edit as a JSON object in `content`:

```json
{"edit_type": "reword", "original": "...", "replacement": "..."}
```

For regular additions (existing behavior), content remains a plain string.

**Step 2: Write the failing test**

Add to `projection/mod.rs` tests (or create the test module):

```rust
#[test]
fn test_edits_to_apply_actions() {
    use crate::models::{ClaudeMdEdit, ClaudeMdEditType};

    let edits = vec![
        ClaudeMdEdit {
            edit_type: ClaudeMdEditType::Reword,
            original_text: "Old text".to_string(),
            suggested_content: Some("New text".to_string()),
            target_section: None,
            reasoning: "clarity".to_string(),
        },
        ClaudeMdEdit {
            edit_type: ClaudeMdEditType::Remove,
            original_text: "Stale rule".to_string(),
            suggested_content: None,
            target_section: None,
            reasoning: "stale".to_string(),
        },
    ];

    let actions = edits_to_apply_actions(&edits, "CLAUDE.md");
    assert_eq!(actions.len(), 2);
    assert_eq!(actions[0].target_type, SuggestedTarget::ClaudeMd);
    assert_eq!(actions[0].track, ApplyTrack::Shared);
    // Content should be JSON-encoded edit
    let parsed: serde_json::Value = serde_json::from_str(&actions[0].content).unwrap();
    assert_eq!(parsed["edit_type"], "reword");
}
```

**Step 3: Write minimal implementation**

Add to `projection/mod.rs`:

```rust
use crate::models::{ClaudeMdEdit, ClaudeMdEditType};

/// Convert claude_md_edits from analysis into ApplyActions for the review queue.
pub fn edits_to_apply_actions(edits: &[ClaudeMdEdit], claude_md_path: &str) -> Vec<ApplyAction> {
    edits
        .iter()
        .map(|edit| {
            let description = match edit.edit_type {
                ClaudeMdEditType::Add => format!("Add: {}", edit.suggested_content.as_deref().unwrap_or("")),
                ClaudeMdEditType::Remove => format!("Remove: {}", edit.original_text),
                ClaudeMdEditType::Reword => format!("Reword: \"{}\"", edit.original_text),
                ClaudeMdEditType::Move => format!("Move: \"{}\"", edit.original_text),
            };

            let content = serde_json::json!({
                "edit_type": edit.edit_type.to_string(),
                "original": edit.original_text,
                "replacement": edit.suggested_content,
                "target_section": edit.target_section,
                "reasoning": edit.reasoning,
            })
            .to_string();

            ApplyAction {
                pattern_id: String::new(), // No backing pattern for direct edits
                pattern_description: description,
                target_type: SuggestedTarget::ClaudeMd,
                target_path: claude_md_path.to_string(),
                content,
                track: ApplyTrack::Shared,
            }
        })
        .collect()
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p retro-core test_edits_to_apply`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/retro-core/src/projection/mod.rs crates/retro-core/src/models.rs
git commit -m "feat: convert claude_md_edits to ApplyActions for review queue"
```

---

### Task 9: Wire edits through `build_apply_plan` and `save_plan_for_review`

**Files:**
- Modify: `crates/retro-core/src/projection/mod.rs`

**Step 1: Design**

`build_apply_plan()` currently only creates actions from patterns. It needs to also accept `claude_md_edits` (from the analysis response) and include them as actions.

The challenge: edits come from the analysis phase, not from stored patterns. Options:
- Store edits in the DB alongside patterns (complex, new schema)
- Pass edits through as a parameter to `build_apply_plan` (simpler)

Go with the simpler approach: store edits temporarily in the DB as a special pattern type, or pass them through. Since `build_apply_plan` is called from `apply.rs` which doesn't have access to analysis results, the cleanest approach is to store the edits in the `patterns` table with a marker (e.g., `suggested_target = "claude_md_edit"`) during analysis, then pick them up in `build_apply_plan`.

Actually, the simplest approach: store edits as patterns with `suggested_target = ClaudeMd` and the edit JSON as `suggested_content`. The `description` field can carry the edit description. This requires no schema changes — edits piggyback on the existing pattern infrastructure.

**Step 2: Write implementation**

In the `analyze()` function, after processing patterns, also store edits as patterns:

```rust
// After merge::process_updates()...
// Store claude_md_edits as pseudo-patterns
for edit in &parsed.claude_md_edits {
    let edit_content = serde_json::json!({
        "edit_type": edit.edit_type.to_string(),
        "original": edit.original_text,
        "replacement": edit.suggested_content,
        "target_section": edit.target_section,
        "reasoning": edit.reasoning,
    });

    let description = match edit.edit_type {
        ClaudeMdEditType::Add => format!("[edit:add] {}", edit.suggested_content.as_deref().unwrap_or("")),
        ClaudeMdEditType::Remove => format!("[edit:remove] {}", edit.original_text),
        ClaudeMdEditType::Reword => format!("[edit:reword] {}", edit.original_text),
        ClaudeMdEditType::Move => format!("[edit:move] {}", edit.original_text),
    };

    let pattern = Pattern {
        id: uuid::Uuid::new_v4().to_string(),
        pattern_type: PatternType::RedundantContext, // closest match
        description,
        confidence: 0.75, // edits are AI-suggested, high-ish confidence
        times_seen: 1,
        first_seen: Utc::now(),
        last_seen: Utc::now(),
        last_projected: None,
        status: PatternStatus::Discovered,
        source_sessions: batch_session_ids.clone(),
        related_files: vec!["CLAUDE.md".to_string()],
        suggested_content: edit_content.to_string(),
        suggested_target: SuggestedTarget::ClaudeMd,
        project: project.map(|s| s.to_string()),
        generation_failed: false,
    };
    db::insert_pattern(conn, &pattern)?;
}
```

Then `build_apply_plan()` automatically picks these up (they have `suggested_target = ClaudeMd` and high enough confidence).

In `execute_plan()`, detect whether a CLAUDE.md action's content is a JSON edit or a plain rule string:

```rust
// When executing CLAUDE.md actions, check if content is a JSON edit
fn is_edit_action(content: &str) -> bool {
    content.starts_with('{') && content.contains("\"edit_type\"")
}
```

**Step 3: Write the test**

```rust
#[test]
fn test_is_edit_action() {
    assert!(is_edit_action(r#"{"edit_type":"reword","original":"x","replacement":"y"}"#));
    assert!(!is_edit_action("Use thiserror in library crates"));
}
```

**Step 4: Run tests**

Run: `cargo test -p retro-core`
Expected: All pass

**Step 5: Commit**

```bash
git add crates/retro-core/src/analysis/mod.rs crates/retro-core/src/projection/mod.rs
git commit -m "feat: store claude_md_edits as patterns and handle in apply plan"
```

---

### Task 10: Update `execute_plan` to handle edit actions

**Files:**
- Modify: `crates/retro-core/src/projection/mod.rs`

**Step 1: Design**

When `execute_plan()` processes CLAUDE.md actions, it currently batches all rules and calls `write_claude_md()`. With edits, it needs to:
1. Separate plain rules (additions) from JSON edits
2. Apply edits via `claude_md::apply_edits()`
3. Apply additions via existing `claude_md::update_claude_md_content()` (or append directly if no managed section)

**Step 2: Write the failing test**

Use a tempdir + mock DB to test the full execute flow with edit actions. This is more of an integration test — add to `projection/mod.rs` tests:

```rust
#[test]
fn test_execute_plan_with_edits() {
    let dir = tempfile::tempdir().unwrap();
    let claude_md_path = dir.path().join("CLAUDE.md");
    std::fs::write(&claude_md_path, "# Project\n\nOld rule\n\nAnother rule\n").unwrap();

    let edit_content = serde_json::json!({
        "edit_type": "reword",
        "original": "Old rule",
        "replacement": "New improved rule",
        "reasoning": "clarity"
    });

    // Test that is_edit_action correctly identifies edit JSON
    assert!(is_edit_action(&edit_content.to_string()));

    // Test that parse_edit extracts the edit correctly
    let edit = parse_edit(&edit_content.to_string()).unwrap();
    assert_eq!(edit.edit_type, ClaudeMdEditType::Reword);
    assert_eq!(edit.original_text, "Old rule");
}
```

**Step 3: Write implementation**

Add helper functions in `projection/mod.rs`:

```rust
/// Check if an action's content is a JSON edit (vs a plain rule string).
pub fn is_edit_action(content: &str) -> bool {
    content.starts_with('{') && content.contains("\"edit_type\"")
}

/// Parse a JSON edit from an action's content field.
pub fn parse_edit(content: &str) -> Option<ClaudeMdEdit> {
    let v: serde_json::Value = serde_json::from_str(content).ok()?;
    Some(ClaudeMdEdit {
        edit_type: match v["edit_type"].as_str()? {
            "add" => ClaudeMdEditType::Add,
            "remove" => ClaudeMdEditType::Remove,
            "reword" => ClaudeMdEditType::Reword,
            "move" => ClaudeMdEditType::Move,
            _ => return None,
        },
        original_text: v["original"].as_str().unwrap_or("").to_string(),
        suggested_content: v["replacement"].as_str().map(|s| s.to_string()),
        target_section: v["target_section"].as_str().map(|s| s.to_string()),
        reasoning: v["reasoning"].as_str().unwrap_or("").to_string(),
    })
}
```

Update `execute_plan()` to handle mixed actions:

```rust
// In execute_plan(), replace the CLAUDE.md block:
if !claude_md_actions.is_empty() {
    let target_path = &claude_md_actions[0].target_path;

    // Separate plain rules from edits
    let mut plain_rules: Vec<String> = Vec::new();
    let mut edits: Vec<ClaudeMdEdit> = Vec::new();

    for action in &claude_md_actions {
        if is_edit_action(&action.content) {
            if let Some(edit) = parse_edit(&action.content) {
                edits.push(edit);
            }
        } else {
            plain_rules.push(action.content.clone());
        }
    }

    // Read existing content
    let existing = std::fs::read_to_string(target_path).unwrap_or_default();
    backup_file(target_path, &backup_dir)?;

    let mut updated = existing.clone();

    // Apply edits first (on the original content)
    if !edits.is_empty() {
        updated = claude_md::apply_edits(&updated, &edits);
    }

    // Then apply plain rule additions
    if !plain_rules.is_empty() {
        updated = claude_md::update_claude_md_content(&updated, &plain_rules);
    }

    std::fs::write(target_path, &updated)?;
    files_written += 1;

    // Record projections...
}
```

**Step 4: Run tests**

Run: `cargo test -p retro-core`
Expected: All pass

**Step 5: Commit**

```bash
git add crates/retro-core/src/projection/mod.rs
git commit -m "feat: execute_plan handles both plain rules and JSON edits"
```

---

### Task 11: Update review command display for edit types

**Files:**
- Modify: `crates/retro-cli/src/commands/review.rs`

**Step 1: Write implementation**

In the display loop in `review.rs` where projections are listed, detect edit actions and show appropriate icons:

```rust
// When displaying each projection item:
let (icon, desc) = if projection::is_edit_action(&proj.content) {
    if let Some(edit) = projection::parse_edit(&proj.content) {
        let icon = match edit.edit_type {
            ClaudeMdEditType::Add => "rule+",
            ClaudeMdEditType::Remove => "rule-",
            ClaudeMdEditType::Reword => "rule~",
            ClaudeMdEditType::Move => "rule>",
        };
        let desc = match edit.edit_type {
            ClaudeMdEditType::Add => format!("Add: {}", edit.suggested_content.unwrap_or_default()),
            ClaudeMdEditType::Remove => format!("Remove: \"{}\" ({})", edit.original_text, edit.reasoning),
            ClaudeMdEditType::Reword => format!("Reword: \"{}\" -> \"{}\"", edit.original_text, edit.suggested_content.unwrap_or_default()),
            ClaudeMdEditType::Move => format!("Move: \"{}\" to {}", edit.original_text, edit.target_section.unwrap_or_default()),
        };
        (icon.to_string(), desc)
    } else {
        ("rule".to_string(), proj.content.clone())
    }
} else {
    // Existing logic for regular projections
    let icon = match proj.target_type.as_str() {
        "skill" => "skill",
        "claude_md" => "rule",
        "global_agent" => "agent",
        _ => "item",
    };
    (icon.to_string(), /* existing description logic */)
};
```

**Step 2: Run full test suite**

Run: `cargo test`
Expected: All pass

**Step 3: Commit**

```bash
git add crates/retro-cli/src/commands/review.rs
git commit -m "feat: display edit type icons in retro review"
```

---

### Task 12: Add `retro curate` CLI command skeleton

**Files:**
- Create: `crates/retro-cli/src/commands/curate.rs`
- Modify: `crates/retro-cli/src/commands/mod.rs`
- Modify: `crates/retro-cli/src/main.rs`

**Step 1: Write the CLI registration**

Add to `main.rs` Commands enum:

```rust
/// AI-powered full CLAUDE.md rewrite (requires full_management = true)
Curate {
    /// Show context summary without making AI calls
    #[arg(long)]
    dry_run: bool,
},
```

Add to the match in `main()`:

```rust
Commands::Curate { dry_run } => commands::curate::run(dry_run, verbose),
```

Add to `commands/mod.rs`:

```rust
pub mod curate;
```

**Step 2: Write the command stub**

Create `crates/retro-cli/src/commands/curate.rs`:

```rust
use anyhow::{bail, Result};
use retro_core::config::{self, Config};

use super::git_root_or_cwd;

pub fn run(dry_run: bool, verbose: bool) -> Result<()> {
    let retro_dir = config::retro_dir();
    let config = Config::load(&retro_dir.join("config.toml"))?;

    if !config.claude_md.full_management {
        bail!(
            "Full CLAUDE.md management is not enabled.\n\
             Set `full_management = true` in [claude_md] section of ~/.retro/config.toml"
        );
    }

    let project_root = git_root_or_cwd()?;

    if dry_run {
        println!("Would gather context for CLAUDE.md rewrite...");
        // TODO: show context summary
        println!("\nDry run — skipping AI call. No changes made.");
        return Ok(());
    }

    println!("retro curate: not yet implemented");
    Ok(())
}
```

**Step 3: Verify it compiles**

Run: `cargo build`
Expected: Compiles successfully

**Step 4: Commit**

```bash
git add crates/retro-cli/src/commands/curate.rs crates/retro-cli/src/commands/mod.rs crates/retro-cli/src/main.rs
git commit -m "feat: add retro curate command skeleton"
```

---

### Task 13: Implement curate context gathering

**Files:**
- Modify: `crates/retro-cli/src/commands/curate.rs`
- Modify: `crates/retro-core/src/analysis/prompts.rs` (new prompt builder)

**Step 1: Write the curate prompt builder**

Add to `prompts.rs`:

```rust
/// Build the seed prompt for `retro curate` — provides context and instructions
/// for an agentic AI call that will explore the codebase and rewrite CLAUDE.md.
pub fn build_curate_prompt(
    claude_md_content: &str,
    patterns: &[Pattern],
    memory_md: Option<&str>,
    project_tree: &str,
) -> String {
    let mut prompt = String::new();

    prompt.push_str("You are rewriting a CLAUDE.md file for a software project. ");
    prompt.push_str("Your goal is to produce the best possible CLAUDE.md that will help AI coding agents ");
    prompt.push_str("work effectively in this codebase.\n\n");

    prompt.push_str("## Current CLAUDE.md\n\n```\n");
    prompt.push_str(claude_md_content);
    prompt.push_str("\n```\n\n");

    if !patterns.is_empty() {
        prompt.push_str("## Discovered Patterns (from session analysis)\n\n");
        for p in patterns {
            prompt.push_str(&format!(
                "- [conf={:.1}] {}: {}\n",
                p.confidence, p.pattern_type, p.description
            ));
            if !p.suggested_content.is_empty() {
                prompt.push_str(&format!("  Content: {}\n", p.suggested_content));
            }
        }
        prompt.push('\n');
    }

    if let Some(memory) = memory_md {
        if !memory.is_empty() {
            prompt.push_str("## MEMORY.md (Claude Code's memory notes)\n\n```\n");
            prompt.push_str(memory);
            prompt.push_str("\n```\n\n");
        }
    }

    prompt.push_str("## Project File Tree\n\n```\n");
    prompt.push_str(project_tree);
    prompt.push_str("\n```\n\n");

    prompt.push_str(r#"## Instructions

1. First, explore the codebase by reading key files to understand the project structure, language, and conventions. Use the project tree above to decide what to read.

2. Then produce a complete, improved CLAUDE.md that:
   - Integrates all confirmed patterns naturally (not as a separate section)
   - Removes stale, redundant, or contradictory rules
   - Reorganizes for clarity and logical grouping
   - Preserves the user's voice and intent
   - Keeps it concise — every line should earn its place
   - Is accurate based on the actual codebase (not just the old CLAUDE.md)

3. Output ONLY the new CLAUDE.md content — no wrapper, no explanation, just the markdown.
   Start directly with the first line of the new CLAUDE.md (usually a # heading).
"#);

    prompt
}
```

**Step 2: Write a test for the prompt builder**

```rust
#[test]
fn test_build_curate_prompt() {
    let prompt = build_curate_prompt(
        "# My Project\nSome rules.",
        &[],
        Some("memory notes"),
        "src/\n  main.rs\nCargo.toml",
    );
    assert!(prompt.contains("# My Project"));
    assert!(prompt.contains("memory notes"));
    assert!(prompt.contains("src/"));
    assert!(prompt.contains("CLAUDE.md"));
}
```

**Step 3: Run test**

Run: `cargo test -p retro-core test_build_curate`
Expected: PASS

**Step 4: Commit**

```bash
git add crates/retro-core/src/analysis/prompts.rs
git commit -m "feat: add build_curate_prompt for retro curate context"
```

---

### Task 14: Add agentic backend execution method

**Files:**
- Modify: `crates/retro-core/src/analysis/backend.rs`
- Modify: `crates/retro-core/src/analysis/claude_cli.rs`

**Step 1: Design**

The curate command needs a different execution mode than the analysis backend:
- No `--json-schema` (output is raw markdown)
- No `--tools ""` (model needs tool access for file reading)
- No `--max-turns` (unlimited)
- Longer timeout (the model may explore for several minutes)

Add an `execute_agentic()` method to `ClaudeCliBackend` (not to the trait — this is specific to the curate use case).

**Step 2: Write the implementation**

Add to `claude_cli.rs`:

```rust
/// Timeout for agentic calls (curate) — much longer than standard analysis.
const AGENTIC_TIMEOUT_SECS: u64 = 600; // 10 minutes

impl ClaudeCliBackend {
    // ... existing methods ...

    /// Execute an agentic prompt — the model has tool access and unlimited turns.
    /// Used by `retro curate` for codebase exploration + CLAUDE.md rewriting.
    /// Returns the raw text output (not JSON-wrapped).
    pub fn execute_agentic(&self, prompt: &str) -> Result<BackendResponse, CoreError> {
        let args = vec![
            "-p",
            "-",
            "--output-format",
            "json",
            "--model",
            &self.model,
        ];
        // No --max-turns (unlimited), no --tools "" (tools enabled), no --json-schema

        let mut child = Command::new("claude")
            .args(&args)
            .env_remove("CLAUDECODE")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| {
                CoreError::Analysis(format!(
                    "failed to spawn claude CLI: {e}. Is claude installed and on PATH?"
                ))
            })?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(prompt.as_bytes()).map_err(|e| {
                CoreError::Analysis(format!("failed to write prompt to claude stdin: {e}"))
            })?;
        }

        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();

        let stdout_handle = thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = stdout_pipe {
                let _ = pipe.read_to_end(&mut buf);
            }
            buf
        });
        let stderr_handle = thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = stderr_pipe {
                let _ = pipe.read_to_end(&mut buf);
            }
            buf
        });

        let timeout = Duration::from_secs(AGENTIC_TIMEOUT_SECS);
        let start = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    if start.elapsed() > timeout {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(CoreError::Analysis(format!(
                            "claude CLI agentic call timed out after {}s",
                            AGENTIC_TIMEOUT_SECS
                        )));
                    }
                    thread::sleep(Duration::from_millis(500));
                }
                Err(e) => {
                    return Err(CoreError::Analysis(format!(
                        "error waiting for claude CLI: {e}"
                    )));
                }
            }
        };

        let stdout_bytes = stdout_handle.join().unwrap_or_default();
        let stderr_bytes = stderr_handle.join().unwrap_or_default();

        if !status.success() {
            let stderr = String::from_utf8_lossy(&stderr_bytes);
            return Err(CoreError::Analysis(format!(
                "claude CLI exited with {}: {}",
                status, stderr
            )));
        }

        let stdout = String::from_utf8_lossy(&stdout_bytes);

        let cli_output: ClaudeCliOutput = serde_json::from_str(&stdout).map_err(|e| {
            CoreError::Analysis(format!(
                "failed to parse claude CLI output: {e}\nraw output: {}",
                truncate_for_error(&stdout)
            ))
        })?;

        if cli_output.is_error {
            let error_text = cli_output.result.unwrap_or_else(|| "unknown error".to_string());
            return Err(CoreError::Analysis(format!(
                "claude CLI returned error: {}", error_text
            )));
        }

        let input_tokens = cli_output.total_input_tokens();
        let output_tokens = cli_output.total_output_tokens();

        // For agentic calls, result is in the `result` field (not structured_output)
        let result_text = cli_output
            .result
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                CoreError::Analysis("claude CLI returned empty result for curate".to_string())
            })?;

        Ok(BackendResponse {
            text: result_text,
            input_tokens,
            output_tokens,
        })
    }
}
```

**Step 3: Verify it compiles**

Run: `cargo build`
Expected: Compiles

**Step 4: Commit**

```bash
git add crates/retro-core/src/analysis/claude_cli.rs
git commit -m "feat: add execute_agentic method for retro curate AI calls"
```

---

### Task 15: Implement full `retro curate` command

**Files:**
- Modify: `crates/retro-cli/src/commands/curate.rs`

**Step 1: Write the full implementation**

```rust
use anyhow::{bail, Context, Result};
use colored::Colorize;
use retro_core::analysis::claude_cli::ClaudeCliBackend;
use retro_core::analysis::prompts;
use retro_core::audit_log;
use retro_core::config::{self, Config};
use retro_core::db;
use retro_core::git;
use retro_core::models::PatternStatus;
use retro_core::util;
use std::io::{self, BufRead, Write};

use super::git_root_or_cwd;

pub fn run(dry_run: bool, verbose: bool) -> Result<()> {
    let retro_dir = config::retro_dir();
    let config = Config::load(&retro_dir.join("config.toml"))?;

    if !config.claude_md.full_management {
        bail!(
            "Full CLAUDE.md management is not enabled.\n\
             Set `full_management = true` in [claude_md] section of ~/.retro/config.toml"
        );
    }

    let project_root = git_root_or_cwd()?;
    let claude_md_path = format!("{}/CLAUDE.md", project_root);

    // Gather seed context
    println!("Gathering context...");

    let claude_md_content = std::fs::read_to_string(&claude_md_path).unwrap_or_default();
    let claude_md_lines = claude_md_content.lines().count();
    println!("  CLAUDE.md: {} lines", claude_md_lines);

    // Load qualifying patterns
    let db_path = retro_dir.join("retro.db");
    let conn = db::open_db(&db_path)?;
    let patterns = db::get_patterns(
        &conn,
        &[PatternStatus::Discovered, PatternStatus::Active],
        Some(&project_root),
    )?;
    let qualifying: Vec<_> = patterns
        .iter()
        .filter(|p| p.confidence >= config.analysis.confidence_threshold)
        .collect();
    println!(
        "  Patterns: {} qualifying (>={} confidence)",
        qualifying.len(),
        config.analysis.confidence_threshold
    );

    // Load MEMORY.md
    let memory_path = config.claude_dir().join("memory").join("MEMORY.md");
    let memory_md = std::fs::read_to_string(&memory_path).ok();
    if let Some(ref m) = memory_md {
        println!("  MEMORY.md: {} lines", m.lines().count());
    }

    // Generate project tree
    let project_tree = generate_project_tree(&project_root);
    let file_count = project_tree.lines().count();
    println!("  Project tree: {} files", file_count);

    if dry_run {
        println!("\nDry run — skipping AI call. No changes made.");
        return Ok(());
    }

    // Auth check
    ClaudeCliBackend::check_auth()?;

    // Build prompt
    let prompt = prompts::build_curate_prompt(
        &claude_md_content,
        &qualifying.iter().map(|p| (*p).clone()).collect::<Vec<_>>(),
        memory_md.as_deref(),
        &project_tree,
    );

    if verbose {
        println!("\nPrompt length: {} chars", prompt.len());
    }

    println!("\nExploring codebase and generating rewrite...");
    println!("(this is an agentic AI call — it may read files and take a minute or two)");

    // Execute agentic call
    let backend = ClaudeCliBackend::new(&config.ai);
    let response = backend.execute_agentic(&prompt)?;

    let new_content = util::strip_code_fences(&response.text);
    let new_lines = new_content.lines().count();

    println!("\nDone. Proposed rewrite: {} lines (was {})", new_lines, claude_md_lines);

    if verbose {
        println!(
            "Tokens: {} in, {} out",
            response.input_tokens, response.output_tokens
        );
    }

    // Show diff
    println!();
    show_unified_diff(&claude_md_content, &new_content);
    println!();

    // Confirmation
    print!(
        "Create a PR with this rewrite? You can review and edit the PR on GitHub\n\
         before merging if you want to tweak anything. [y/N] "
    );
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().lock().read_line(&mut input)?;

    if input.trim().to_lowercase() != "y" {
        println!("Rewrite discarded. No changes made.");
        audit_log::append(
            &retro_dir.join("audit.jsonl"),
            "curate_rejected",
            serde_json::json!({
                "lines_before": claude_md_lines,
                "lines_after": new_lines,
                "input_tokens": response.input_tokens,
                "output_tokens": response.output_tokens,
            }),
        )?;
        return Ok(());
    }

    // Create PR
    let backup_dir = retro_dir.join("backups");
    util::backup_file(&claude_md_path, &backup_dir)?;

    let default_branch = git::default_branch()?;
    git::fetch_branch(&default_branch)?;

    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let branch_name = format!("retro/curate-{}", timestamp);

    let stashed = git::stash_push()?;

    git::create_branch(&branch_name, Some(&format!("origin/{}", default_branch)))?;

    std::fs::write(&claude_md_path, &new_content)
        .context("writing rewritten CLAUDE.md")?;

    let commit_msg = "retro: curate CLAUDE.md\n\nAI-powered rewrite via retro curate.";
    git::commit_files(&["CLAUDE.md"], commit_msg)?;
    git::push_current_branch()?;

    let title = "retro: curate CLAUDE.md";
    let body = format!(
        "## CLAUDE.md Rewrite\n\n\
         AI-powered rewrite of CLAUDE.md via `retro curate`.\n\n\
         - Lines: {} -> {}\n\
         - Tokens: {} in, {} out\n\n\
         Review the diff, edit if needed, then merge.\n\n\
         ---\nGenerated by `retro curate`.",
        claude_md_lines, new_lines, response.input_tokens, response.output_tokens
    );
    let pr_url = git::create_pr(title, &body, &default_branch)?;

    // Switch back to original branch
    git::checkout_branch(&default_branch)?;
    if stashed {
        git::stash_pop()?;
    }

    println!(
        "\n{} {}",
        "PR created:".green().bold(),
        pr_url
    );
    println!("Edit the PR on GitHub if needed, then merge when ready.");

    // Audit log
    audit_log::append(
        &retro_dir.join("audit.jsonl"),
        "curate_applied",
        serde_json::json!({
            "pr_url": pr_url,
            "lines_before": claude_md_lines,
            "lines_after": new_lines,
            "input_tokens": response.input_tokens,
            "output_tokens": response.output_tokens,
        }),
    )?;

    Ok(())
}

/// Generate a filtered project tree (exclude build artifacts, .git, etc.)
fn generate_project_tree(root: &str) -> String {
    let output = std::process::Command::new("find")
        .args([
            root, "-type", "f",
            "-not", "-path", "*/.git/*",
            "-not", "-path", "*/target/*",
            "-not", "-path", "*/node_modules/*",
            "-not", "-path", "*/__pycache__/*",
            "-not", "-path", "*/.venv/*",
            "-not", "-path", "*/dist/*",
            "-not", "-path", "*/.next/*",
            "-not", "-name", "*.lock",
            "-not", "-name", "*.pyc",
        ])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let tree = String::from_utf8_lossy(&out.stdout);
            // Make paths relative to root
            tree.lines()
                .map(|l| l.strip_prefix(root).unwrap_or(l).trim_start_matches('/'))
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        }
        _ => "Could not generate project tree".to_string(),
    }
}

/// Show a simple unified diff between old and new content.
fn show_unified_diff(old: &str, new: &str) {
    // Use a simple line-by-line diff display
    // For a real impl, consider using the `similar` crate or shelling out to `diff`
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    println!("{}", "--- CLAUDE.md (current)".red());
    println!("{}", "+++ CLAUDE.md (proposed)".green());

    // Simple approach: shell out to diff for proper unified diff
    let tmp_old = std::env::temp_dir().join("retro-curate-old.md");
    let tmp_new = std::env::temp_dir().join("retro-curate-new.md");
    let _ = std::fs::write(&tmp_old, old);
    let _ = std::fs::write(&tmp_new, new);

    if let Ok(output) = std::process::Command::new("diff")
        .args(["-u", "--color=always"])
        .arg(&tmp_old)
        .arg(&tmp_new)
        .output()
    {
        let diff = String::from_utf8_lossy(&output.stdout);
        // Skip the first two lines (--- and +++ with temp paths) since we already printed headers
        for line in diff.lines().skip(2) {
            println!("{}", line);
        }
    } else {
        // Fallback: just show line counts
        println!("  {} lines removed, {} lines added",
            old_lines.len(), new_lines.len());
    }

    let _ = std::fs::remove_file(&tmp_old);
    let _ = std::fs::remove_file(&tmp_new);
}
```

**Step 2: Verify it compiles**

Run: `cargo build`
Expected: Compiles

**Step 3: Commit**

```bash
git add crates/retro-cli/src/commands/curate.rs
git commit -m "feat: implement retro curate command — agentic CLAUDE.md rewrite with PR"
```

---

### Task 16: Add delimiter dissolution to the apply/curate flow

**Files:**
- Modify: `crates/retro-cli/src/commands/curate.rs`
- Modify: `crates/retro-core/src/projection/mod.rs`

**Step 1: Design**

When `full_management = true`, before the first analysis or curate run:
1. Check if CLAUDE.md has managed delimiters
2. If yes, dissolve them (strip markers, keep content)
3. Write the cleaned file back

This should happen once, early in the flow.

**Step 2: Write implementation**

Add a helper in `projection/mod.rs`:

```rust
/// If full_management is enabled and CLAUDE.md has managed delimiters, dissolve them.
/// Returns true if dissolution happened.
pub fn dissolve_if_needed(claude_md_path: &str, backup_dir: &Path) -> Result<bool, CoreError> {
    let content = match std::fs::read_to_string(claude_md_path) {
        Ok(c) => c,
        Err(_) => return Ok(false),
    };

    if !claude_md::has_managed_section(&content) {
        return Ok(false);
    }

    util::backup_file(claude_md_path, backup_dir)?;
    let dissolved = claude_md::dissolve_managed_section(&content);
    std::fs::write(claude_md_path, &dissolved)
        .map_err(|e| CoreError::Io(format!("writing dissolved CLAUDE.md: {e}")))?;
    Ok(true)
}
```

Call this at the start of `curate::run()` and in `apply::run_apply()` when `full_management` is enabled.

**Step 3: Write test**

```rust
#[test]
fn test_dissolve_if_needed() {
    let dir = tempfile::tempdir().unwrap();
    let claude_md = dir.path().join("CLAUDE.md");
    let backup_dir = dir.path().join("backups");
    std::fs::create_dir_all(&backup_dir).unwrap();

    // With managed section
    std::fs::write(&claude_md, "# Proj\n\n<!-- retro:managed:start -->\n## Retro-Discovered Patterns\n\n- Rule\n\n<!-- retro:managed:end -->\n").unwrap();
    let dissolved = dissolve_if_needed(claude_md.to_str().unwrap(), &backup_dir).unwrap();
    assert!(dissolved);
    let content = std::fs::read_to_string(&claude_md).unwrap();
    assert!(!content.contains("retro:managed"));
    assert!(content.contains("- Rule"));

    // Without managed section (already dissolved)
    let dissolved2 = dissolve_if_needed(claude_md.to_str().unwrap(), &backup_dir).unwrap();
    assert!(!dissolved2);
}
```

**Step 4: Run tests**

Run: `cargo test -p retro-core test_dissolve_if`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/retro-core/src/projection/mod.rs crates/retro-cli/src/commands/curate.rs
git commit -m "feat: dissolve managed delimiters when full_management is enabled"
```

---

### Task 17: Update CLAUDE.md with new feature documentation

**Files:**
- Modify: `CLAUDE.md`

**Step 1: Add documentation**

Add to the "Key Design Decisions" section:

```markdown
- **Full CLAUDE.md management** — opt-in via `[claude_md] full_management = true`. Two modes: (1) granular edits (add/remove/reword/move) through the analysis → review queue pipeline, (2) `retro curate` for agentic full rewrites via PR. Managed delimiters dissolved on first run. Language-agnostic — the AI explores the codebase to understand project structure.
```

Add `retro curate` to the Implementation Status section.

**Step 2: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: add full CLAUDE.md management to project documentation"
```

---

### Task 18: Run full test suite and fix any issues

**Step 1: Run all tests**

Run: `cargo test`
Expected: All tests pass

**Step 2: Run clippy**

Run: `cargo clippy --all-targets`
Expected: No warnings

**Step 3: Build release**

Run: `cargo build --release`
Expected: Compiles clean

**Step 4: Fix any issues discovered**

Address any compilation errors, test failures, or clippy warnings.

**Step 5: Final commit**

```bash
git add -A
git commit -m "fix: address test/clippy issues from full CLAUDE.md management feature"
```
