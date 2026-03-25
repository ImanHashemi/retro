use crate::config::TrustConfig;
use crate::models::{NodeType, NodeScope};

/// Determine if a suggestion should be auto-approved based on trust config.
pub fn should_auto_approve(config: &TrustConfig, node_type: &NodeType, scope: &NodeScope) -> bool {
    if config.mode == "review" {
        return false;
    }

    let scope_allowed = match scope {
        NodeScope::Global => config.scope.global_changes == "auto",
        NodeScope::Project => config.scope.project_changes == "auto",
    };

    if !scope_allowed {
        return false;
    }

    match node_type {
        NodeType::Rule => config.auto_approve.rules,
        NodeType::Skill => config.auto_approve.skills,
        NodeType::Preference => config.auto_approve.preferences,
        NodeType::Directive => config.auto_approve.directives,
        NodeType::Pattern => config.auto_approve.rules,
        NodeType::Memory => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{TrustConfig, AutoApproveConfig, TrustScopeConfig};
    use crate::models::{NodeType, NodeScope};

    fn review_config() -> TrustConfig {
        TrustConfig {
            mode: "review".to_string(),
            auto_approve: AutoApproveConfig {
                rules: true, skills: false, preferences: true, directives: true,
            },
            scope: TrustScopeConfig {
                global_changes: "review".to_string(),
                project_changes: "auto".to_string(),
            },
        }
    }

    fn auto_config() -> TrustConfig {
        TrustConfig {
            mode: "auto".to_string(),
            auto_approve: AutoApproveConfig {
                rules: true, skills: false, preferences: true, directives: true,
            },
            scope: TrustScopeConfig {
                global_changes: "review".to_string(),
                project_changes: "auto".to_string(),
            },
        }
    }

    #[test]
    fn test_review_mode_never_auto_approves() {
        let config = review_config();
        assert!(!should_auto_approve(&config, &NodeType::Rule, &NodeScope::Project));
        assert!(!should_auto_approve(&config, &NodeType::Directive, &NodeScope::Global));
    }

    #[test]
    fn test_auto_mode_respects_type_config() {
        let config = auto_config();
        assert!(should_auto_approve(&config, &NodeType::Rule, &NodeScope::Project));
        assert!(!should_auto_approve(&config, &NodeType::Skill, &NodeScope::Project));
        assert!(should_auto_approve(&config, &NodeType::Preference, &NodeScope::Project));
    }

    #[test]
    fn test_auto_mode_respects_scope_config() {
        let config = auto_config();
        assert!(!should_auto_approve(&config, &NodeType::Rule, &NodeScope::Global));
        assert!(should_auto_approve(&config, &NodeType::Rule, &NodeScope::Project));
    }
}
