use anyhow::Result;
use chrono::{Duration, Utc};
use colored::Colorize;
use retro_core::audit_log;
use retro_core::config::retro_dir;

pub fn run(since: Option<String>) -> Result<()> {
    let dir = retro_dir();
    let audit_path = dir.join("audit.jsonl");

    if !audit_path.exists() {
        println!("{}", "No audit log found. Run `retro analyze` or `retro apply` first.".yellow());
        return Ok(());
    }

    // Parse --since value (e.g., "7d", "30d", "24h")
    let since_time = match &since {
        Some(s) => Some(parse_duration_str(s)?),
        None => None,
    };

    let entries = audit_log::read_entries(&audit_path, since_time.as_ref())?;

    if entries.is_empty() {
        let msg = match &since {
            Some(s) => format!("No audit log entries found in the last {s}."),
            None => "No audit log entries found.".to_string(),
        };
        println!("{}", msg.yellow());
        return Ok(());
    }

    println!(
        "{} ({} entries):",
        "Audit Log".bold(),
        entries.len().to_string().cyan()
    );
    println!();

    for entry in &entries {
        let time_str = entry.timestamp.format("%Y-%m-%d %H:%M:%S UTC");
        let action_colored = match entry.action.as_str() {
            "analyze" => entry.action.cyan(),
            "apply" => entry.action.green(),
            "clean" => entry.action.yellow(),
            "audit" => entry.action.magenta(),
            _ => entry.action.white(),
        };

        println!("  {} {}", time_str.to_string().dimmed(), action_colored);

        // Print relevant details based on action type
        if let Some(obj) = entry.details.as_object() {
            let mut detail_parts = Vec::new();

            for (key, value) in obj {
                // Skip verbose fields
                if key == "finding_types" {
                    continue;
                }
                let display = match value {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::Bool(b) => b.to_string(),
                    serde_json::Value::Null => "null".to_string(),
                    _ => serde_json::to_string(value).unwrap_or_default(),
                };
                detail_parts.push(format!("{key}={display}"));
            }

            if !detail_parts.is_empty() {
                println!("    {}", detail_parts.join(", ").dimmed());
            }
        }
    }

    Ok(())
}

/// Parse duration strings like "7d", "30d", "24h" into a DateTime.
pub(crate) fn parse_duration_str(s: &str) -> Result<chrono::DateTime<Utc>> {
    let s = s.trim();

    if let Some(days) = s.strip_suffix('d') {
        let n: i64 = days
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid duration: {s}"))?;
        Ok(Utc::now() - Duration::days(n))
    } else if let Some(hours) = s.strip_suffix('h') {
        let n: i64 = hours
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid duration: {s}"))?;
        Ok(Utc::now() - Duration::hours(n))
    } else {
        // Default to days
        let n: i64 = s
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid duration: {s}. Use format like '7d' or '24h'"))?;
        Ok(Utc::now() - Duration::days(n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration_days() {
        let before = Utc::now();
        let result = parse_duration_str("7d").unwrap();
        let expected = before - Duration::days(7);
        assert!((result - expected).num_seconds().abs() < 2);
    }

    #[test]
    fn test_parse_duration_hours() {
        let before = Utc::now();
        let result = parse_duration_str("24h").unwrap();
        let expected = before - Duration::hours(24);
        assert!((result - expected).num_seconds().abs() < 2);
    }

    #[test]
    fn test_parse_duration_bare_number() {
        let before = Utc::now();
        let result = parse_duration_str("30").unwrap();
        let expected = before - Duration::days(30);
        assert!((result - expected).num_seconds().abs() < 2);
    }

    #[test]
    fn test_parse_duration_invalid() {
        assert!(parse_duration_str("abc").is_err());
        assert!(parse_duration_str("7x").is_err());
    }

    #[test]
    fn test_parse_duration_whitespace() {
        let before = Utc::now();
        let result = parse_duration_str("  3d  ").unwrap();
        let expected = before - Duration::days(3);
        assert!((result - expected).num_seconds().abs() < 2);
    }
}
