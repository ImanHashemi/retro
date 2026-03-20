use crate::config;
use crate::errors::CoreError;

/// Generate a briefing markdown string for a project.
/// Returns empty string if there's nothing to report.
pub fn generate_briefing(
    _project_id: &str,
    applied: &[String],
    learned: &[String],
    pending_count: usize,
) -> String {
    if applied.is_empty() && learned.is_empty() && pending_count == 0 {
        return String::new();
    }

    let mut content = String::new();
    content.push_str("## What's new since your last session\n\n");

    for item in applied {
        content.push_str(&format!("- **Applied:** {item}\n"));
    }
    for item in learned {
        content.push_str(&format!("- **Learned:** {item}\n"));
    }
    if pending_count > 0 {
        content.push_str(&format!(
            "- **Pending:** {pending_count} suggestion{} waiting for your review — run `retro dash`\n",
            if pending_count == 1 { "" } else { "s" }
        ));
    }

    content
}

/// Write a briefing file to ~/.retro/briefings/<project_id>.md
pub fn write_briefing(project_id: &str, content: &str) -> Result<(), CoreError> {
    let briefings_dir = config::retro_dir().join("briefings");
    std::fs::create_dir_all(&briefings_dir)
        .map_err(|e| CoreError::Io(format!("creating briefings dir: {e}")))?;

    let path = briefings_dir.join(format!("{project_id}.md"));
    if content.is_empty() {
        let _ = std::fs::remove_file(&path);
    } else {
        std::fs::write(&path, content)
            .map_err(|e| CoreError::Io(format!("writing briefing: {e}")))?;
    }
    Ok(())
}

/// Read a briefing file if it exists.
pub fn read_briefing(project_id: &str) -> Option<String> {
    let path = config::retro_dir()
        .join("briefings")
        .join(format!("{project_id}.md"));
    std::fs::read_to_string(&path).ok().filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_briefing_with_applied() {
        let briefing = generate_briefing(
            "my-app",
            &["Added rule: Always run tests".to_string()],
            &[],
            0,
        );
        assert!(briefing.contains("What's new since your last session"));
        assert!(briefing.contains("Applied"));
        assert!(briefing.contains("Always run tests"));
    }

    #[test]
    fn test_generate_briefing_with_pending() {
        let briefing = generate_briefing("my-app", &[], &[], 3);
        assert!(briefing.contains("Pending"));
        assert!(briefing.contains("3"));
        assert!(briefing.contains("retro dash"));
    }

    #[test]
    fn test_generate_briefing_empty() {
        let briefing = generate_briefing("my-app", &[], &[], 0);
        assert!(briefing.is_empty());
    }
}
