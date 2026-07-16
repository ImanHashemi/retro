/// v3 session briefing: notifications (new registrations, learned rules) plus
/// health warnings. Empty inputs produce an empty string (hook prints nothing).
pub fn build_v3_briefing(notifications: &[String], health_warnings: &[String]) -> String {
    if notifications.is_empty() && health_warnings.is_empty() {
        return String::new();
    }
    let mut out = String::from("Retro update — mention briefly to the user at conversation start.\n");
    for n in notifications {
        out.push_str(&format!("- {n}\n"));
    }
    for w in health_warnings {
        out.push_str(&format!("⚠ {w}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v3_briefing_formats_sections_and_empties_to_empty() {
        assert_eq!(build_v3_briefing(&[], &[]), "");
        let out = build_v3_briefing(
            &["retro is now watching `my-proj`".to_string(), "Learned: always smoke test".to_string()],
            &["retro analyze failed at 2026-07-06T10:00:00Z: exit 1".to_string()],
        );
        assert!(out.starts_with("Retro update"));
        assert!(out.contains("- retro is now watching `my-proj`"));
        assert!(out.contains("- Learned: always smoke test"));
        assert!(out.contains("⚠ retro analyze failed"));
    }
}
