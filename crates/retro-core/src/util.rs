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
}
