use crate::errors::CoreError;
use chrono::Utc;
use std::path::Path;

/// Backup a file to the backup directory.
/// Uses a sanitized path to avoid collisions between files with the same name
/// in different directories (e.g., /proj-a/CLAUDE.md vs /proj-b/CLAUDE.md).
pub fn backup_file(path: &str, backup_dir: &Path) -> Result<(), CoreError> {
    if !Path::new(path).exists() {
        return Ok(());
    }

    let sanitized = path
        .replace(['/', '\\'], "_")
        .trim_start_matches('_')
        .to_string();

    let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
    let backup_path = backup_dir.join(format!("{sanitized}.{timestamp}.bak"));

    std::fs::copy(path, &backup_path).map_err(|e| {
        CoreError::Io(format!(
            "backing up {} to {}: {e}",
            path,
            backup_path.display()
        ))
    })?;

    Ok(())
}

/// Truncate a string at a valid UTF-8 char boundary. Never panics.
pub fn truncate_str(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut i = max;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    &s[..i]
}

/// Shorten a path for display: replace home directory prefix with `~`.
pub fn shorten_path(path: &str) -> String {
    if let Some(home) = std::env::var_os("HOME") {
        let home_str = home.to_string_lossy();
        if path.starts_with(home_str.as_ref()) {
            return format!("~{}", &path[home_str.len()..]);
        }
    }
    path.to_string()
}

/// Shorten a `Path` for display: replace home directory prefix with `~`.
pub fn shorten_path_buf(path: &std::path::Path) -> String {
    shorten_path(&path.display().to_string())
}

/// Strip markdown code fences from an AI response.
/// Handles ```json, ```yaml, ```markdown, and bare ``` fences.
/// Returns the inner content if fences are found, otherwise returns the input trimmed.
pub fn strip_code_fences(content: &str) -> String {
    let trimmed = content.trim();
    if !trimmed.starts_with("```") {
        return trimmed.to_string();
    }

    let lines: Vec<&str> = trimmed.lines().collect();
    let mut result = Vec::new();
    let mut in_block = false;

    for line in lines {
        if line.starts_with("```") && !in_block {
            in_block = true;
            continue;
        }
        if line.starts_with("```") && in_block {
            break;
        }
        if in_block {
            result.push(line);
        }
    }

    if result.is_empty() {
        trimmed.to_string()
    } else {
        result.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_json_fences() {
        let input = "```json\n{\"key\": \"value\"}\n```";
        assert_eq!(strip_code_fences(input), "{\"key\": \"value\"}");
    }

    #[test]
    fn test_strip_yaml_fences() {
        let input = "```yaml\n---\nname: test\n---\nbody\n```";
        assert_eq!(strip_code_fences(input), "---\nname: test\n---\nbody");
    }

    #[test]
    fn test_strip_bare_fences() {
        let input = "```\ncontent here\n```";
        assert_eq!(strip_code_fences(input), "content here");
    }

    #[test]
    fn test_no_fences() {
        let input = "just plain text";
        assert_eq!(strip_code_fences(input), "just plain text");
    }

    #[test]
    fn test_whitespace_trimmed() {
        let input = "  \n```json\n{}\n```\n  ";
        assert_eq!(strip_code_fences(input), "{}");
    }

    #[test]
    fn test_truncate_str_ascii() {
        assert_eq!(truncate_str("hello world", 5), "hello");
    }

    #[test]
    fn test_truncate_str_no_truncation() {
        assert_eq!(truncate_str("short", 100), "short");
    }

    #[test]
    fn test_truncate_str_utf8_boundary() {
        // "café" is 5 bytes: c(1) a(1) f(1) é(2)
        let s = "caf\u{00e9}!";
        // Truncating at byte 4 would land mid-é, should walk back to 3
        assert_eq!(truncate_str(s, 4), "caf");
    }

    #[test]
    fn test_truncate_str_empty() {
        assert_eq!(truncate_str("", 10), "");
    }

    #[test]
    fn test_shorten_path_replaces_home() {
        let home = std::env::var("HOME").unwrap();
        let input = format!("{home}/projects/foo");
        assert_eq!(shorten_path(&input), "~/projects/foo");
    }

    #[test]
    fn test_shorten_path_no_home_prefix() {
        assert_eq!(shorten_path("/tmp/foo"), "/tmp/foo");
    }

    #[test]
    fn test_shorten_path_buf_works() {
        let home = std::env::var("HOME").unwrap();
        let p = std::path::PathBuf::from(format!("{home}/.retro/retro.db"));
        assert_eq!(shorten_path_buf(&p), "~/.retro/retro.db");
    }
}
