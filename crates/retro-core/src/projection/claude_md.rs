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

/// Remove the managed block (markers inclusive) plus one adjacent trailing
/// blank line; user content around it is untouched. No block -> unchanged.
pub fn strip_managed_section(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let Some(start) = lines.iter().position(|l| l.trim() == MANAGED_START) else {
        return content.to_string();
    };
    let Some(end_rel) = lines[start..].iter().position(|l| l.trim() == MANAGED_END) else {
        return content.to_string();
    };
    let mut end = start + end_rel;
    if lines.get(end + 1).map(|l| l.trim().is_empty()).unwrap_or(false) {
        end += 1;
    }
    let mut out: Vec<&str> = Vec::new();
    out.extend(&lines[..start]);
    out.extend(&lines[end + 1..]);
    let mut s = out.join("\n");
    if content.ends_with('\n') && !s.ends_with('\n') {
        s.push('\n');
    }
    s
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
    fn test_has_managed_section() {
        let with = format!("content\n{}\nrules\n{}\n", MANAGED_START, MANAGED_END);
        let without = "just content\n";
        assert!(has_managed_section(&with));
        assert!(!has_managed_section(without));
    }

    #[test]
    fn strip_managed_section_removes_block_keeps_user_content() {
        let content = "# Mine\n\n<!-- retro:managed:start -->\n- a rule\n<!-- retro:managed:end -->\n\n## Also mine\n";
        let out = strip_managed_section(content);
        assert!(out.contains("# Mine") && out.contains("## Also mine"));
        assert!(!out.contains("retro:managed") && !out.contains("a rule"));
        assert_eq!(strip_managed_section("no block here\n"), "no block here\n");
        // unclosed block: leave the file alone rather than guess at bounds
        let unclosed = "<!-- retro:managed:start -->\n- orphan\n";
        assert_eq!(strip_managed_section(unclosed), unclosed);
    }
}
