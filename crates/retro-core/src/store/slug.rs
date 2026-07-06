//! Kebab-case slug generation for node ids and project directory names.

/// Lowercase ASCII-alphanumeric kebab-case, dashes collapsed, max 60 chars.
/// Falls back to "node" for inputs with no usable characters.
pub fn slugify(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut prev_dash = true; // suppress leading dash
    for c in input.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
        if out.len() >= 60 {
            break;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "node".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("AB Paired Observations"), "ab-paired-observations");
        assert_eq!(slugify("use_pytest fixtures!"), "use-pytest-fixtures");
        assert_eq!(slugify("already-kebab-case"), "already-kebab-case");
    }

    #[test]
    fn slugify_collapses_and_trims_dashes() {
        assert_eq!(slugify("--weird   input--"), "weird-input");
        assert_eq!(slugify("a///b"), "a-b");
    }

    #[test]
    fn slugify_drops_non_ascii() {
        assert_eq!(slugify("café rules ☕"), "caf-rules");
    }

    #[test]
    fn slugify_caps_length_at_60() {
        let long = "x".repeat(100);
        assert_eq!(slugify(&long).len(), 60);
    }

    #[test]
    fn slugify_empty_and_symbol_only_fall_back() {
        assert_eq!(slugify(""), "node");
        assert_eq!(slugify("!!!"), "node");
    }
}
