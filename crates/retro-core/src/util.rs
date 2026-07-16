use crate::errors::CoreError;
use chrono::Utc;
use std::fs::OpenOptions;
use std::io::Write;
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

    // Callers pass directories that may not exist yet (e.g. <store>/backups/
    // on a fresh v3 store) — a bare copy would fail every backup, and with it
    // the projection that requested the backup.
    std::fs::create_dir_all(backup_dir)
        .map_err(|e| CoreError::Io(format!("creating {}: {e}", backup_dir.display())))?;

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

/// Log a parse warning to ~/.retro/warnings.log instead of stderr.
/// Best-effort: silently drops the message if the file can't be opened.
pub fn log_parse_warning(msg: &str) {
    let log_path = crate::config::retro_dir().join("warnings.log");
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&log_path) {
        let ts = Utc::now().format("%Y-%m-%dT%H:%M:%S");
        let _ = writeln!(file, "[{ts}] {msg}");
    }
}

/// Compute normalized Levenshtein similarity between two strings.
/// Returns a value in [0.0, 1.0] where 1.0 means identical.
pub fn normalized_similarity(a: &str, b: &str) -> f64 {
    let a_chars: Vec<char> = a.to_lowercase().chars().collect();
    let b_chars: Vec<char> = b.to_lowercase().chars().collect();
    let a_len = a_chars.len();
    let b_len = b_chars.len();

    let max_len = std::cmp::max(a_len, b_len);
    if max_len == 0 {
        return 1.0;
    }

    let distance = levenshtein(&a_chars, &b_chars);
    1.0 - (distance as f64 / max_len as f64)
}

/// Levenshtein edit distance between two character slices.
pub fn levenshtein(a: &[char], b: &[char]) -> usize {
    let a_len = a.len();
    let b_len = b.len();

    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    // Two-row optimization
    let mut prev: Vec<usize> = (0..=b_len).collect();
    let mut curr = vec![0; b_len + 1];

    for (i, a_ch) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, b_ch) in b.iter().enumerate() {
            let cost = if a_ch == b_ch { 0 } else { 1 };
            curr[j + 1] = std::cmp::min(
                std::cmp::min(prev[j + 1] + 1, curr[j] + 1),
                prev[j] + cost,
            );
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[b_len]
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
    fn test_identical_strings() {
        assert!((normalized_similarity("hello", "hello") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_completely_different() {
        let sim = normalized_similarity("abc", "xyz");
        assert!(sim < 0.5);
    }

    #[test]
    fn test_similar_strings() {
        let sim = normalized_similarity(
            "Always use uv for Python packages",
            "Always use uv for Python package management",
        );
        assert!(sim > 0.7);
    }

    #[test]
    fn test_empty_strings() {
        assert!((normalized_similarity("", "") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_case_insensitive() {
        assert!((normalized_similarity("Hello World", "hello world") - 1.0).abs() < f64::EPSILON);
    }

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
