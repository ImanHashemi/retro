const MANAGED_START: &str = "<!-- retro:managed:start -->";
const MANAGED_END: &str = "<!-- retro:managed:end -->";

/// Build the managed section content from a list of rules.
pub fn build_managed_section(rules: &[String]) -> String {
    let mut section = String::new();
    section.push_str(MANAGED_START);
    section.push('\n');
    section.push_str("## Retro-Discovered Patterns\n\n");
    for rule in rules {
        section.push_str(&format!("- {rule}\n"));
    }
    section.push('\n');
    section.push_str(MANAGED_END);
    section
}

/// Update CLAUDE.md content, inserting or replacing the managed section.
/// Never touches content outside the managed delimiters.
pub fn update_claude_md_content(existing: &str, rules: &[String]) -> String {
    let managed = build_managed_section(rules);

    if let Some((before, after)) = find_managed_bounds(existing) {
        // Replace existing managed section
        format!("{before}{managed}{after}")
    } else {
        // Append managed section at the end
        let mut result = existing.to_string();
        if !result.is_empty() && !result.ends_with('\n') {
            result.push('\n');
        }
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(&managed);
        result.push('\n');
        result
    }
}

/// Extract the current managed section content (rules only, no delimiters).
pub fn read_managed_section(content: &str) -> Option<Vec<String>> {
    let (_, inner, _) = split_managed(content)?;
    let rules: Vec<String> = inner
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("- ") {
                Some(rest.to_string())
            } else {
                None
            }
        })
        .collect();
    if rules.is_empty() {
        None
    } else {
        Some(rules)
    }
}

/// Split content into (before_start_marker, between_markers, after_end_marker).
fn split_managed(content: &str) -> Option<(String, String, String)> {
    let start_idx = content.find(MANAGED_START)?;
    let after_start = start_idx + MANAGED_START.len();

    let end_idx = content[after_start..].find(MANAGED_END)?;
    let end_abs = after_start + end_idx;
    let after_end = end_abs + MANAGED_END.len();

    Some((
        content[..start_idx].to_string(),
        content[after_start..end_abs].to_string(),
        content[after_end..].to_string(),
    ))
}

/// Find managed section bounds, returning (content before start marker, content after end marker).
fn find_managed_bounds(content: &str) -> Option<(String, String)> {
    let (before, _, after) = split_managed(content)?;
    Some((before, after))
}

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

    // Strip the "## Retro-Discovered Patterns" header from inner content
    let cleaned_inner: String = inner
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            trimmed != "## Retro-Discovered Patterns"
        })
        .collect::<Vec<_>>()
        .join("\n");

    let mut result = before;
    if !cleaned_inner.trim().is_empty() {
        result.push_str(&cleaned_inner);
    }
    result.push_str(&after);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_managed_section() {
        let rules = vec![
            "Always use uv for Python packages".to_string(),
            "Run cargo test after changes".to_string(),
        ];
        let section = build_managed_section(&rules);
        assert!(section.starts_with(MANAGED_START));
        assert!(section.ends_with(MANAGED_END));
        assert!(section.contains("- Always use uv for Python packages"));
        assert!(section.contains("- Run cargo test after changes"));
    }

    #[test]
    fn test_update_claude_md_no_existing_section() {
        let existing = "# My Project\n\nSome existing content.\n";
        let rules = vec!["Use uv".to_string()];
        let result = update_claude_md_content(existing, &rules);

        assert!(result.starts_with("# My Project\n\nSome existing content.\n"));
        assert!(result.contains(MANAGED_START));
        assert!(result.contains("- Use uv"));
        assert!(result.contains(MANAGED_END));
    }

    #[test]
    fn test_update_claude_md_replace_existing() {
        let existing = format!(
            "# My Project\n\n{}\n## Retro-Discovered Patterns\n\n- Old rule\n\n{}\n\n## Footer\n",
            MANAGED_START, MANAGED_END
        );
        let rules = vec!["New rule".to_string()];
        let result = update_claude_md_content(&existing, &rules);

        assert!(result.contains("# My Project"));
        assert!(result.contains("- New rule"));
        assert!(!result.contains("- Old rule"));
        assert!(result.contains("## Footer"));
    }

    #[test]
    fn test_update_claude_md_empty_file() {
        let rules = vec!["Rule one".to_string()];
        let result = update_claude_md_content("", &rules);
        assert!(result.contains(MANAGED_START));
        assert!(result.contains("- Rule one"));
    }

    #[test]
    fn test_read_managed_section() {
        let content = format!(
            "# Header\n\n{}\n## Retro-Discovered Patterns\n\n- Rule A\n- Rule B\n\n{}\n",
            MANAGED_START, MANAGED_END
        );
        let rules = read_managed_section(&content).unwrap();
        assert_eq!(rules, vec!["Rule A", "Rule B"]);
    }

    #[test]
    fn test_read_managed_section_none() {
        let content = "# No managed section here\n";
        assert!(read_managed_section(content).is_none());
    }

    #[test]
    fn test_dissolve_managed_section() {
        let content = format!(
            "# My Project\n\nSome content.\n\n{}\n## Retro-Discovered Patterns\n\n- Rule A\n- Rule B\n\n{}\n\n## Footer\n",
            MANAGED_START, MANAGED_END
        );
        let result = dissolve_managed_section(&content);
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
}
