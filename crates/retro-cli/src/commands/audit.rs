use anyhow::Result;
use colored::Colorize;
use retro_core::analysis::claude_cli::ClaudeCliBackend;
use retro_core::analysis::prompts;
use retro_core::audit_log;
use retro_core::config::{retro_dir, Config};
use retro_core::curator::AuditResponse;
use retro_core::ingest::context::snapshot_context;
use retro_core::lock::LockFile;
use retro_core::util::{strip_code_fences, truncate_str};

use super::git_root_or_cwd;

pub fn run(dry_run: bool, verbose: bool) -> Result<()> {
    let dir = retro_dir();
    let config_path = dir.join("config.toml");
    let db_path = dir.join("retro.db");
    let audit_path = dir.join("audit.jsonl");
    let lock_path = dir.join("retro.lock");

    if !db_path.exists() {
        anyhow::bail!("retro not initialized. Run `retro init` first.");
    }

    let config = Config::load(&config_path)?;

    let _lock = LockFile::acquire(&lock_path)
        .map_err(|e| anyhow::anyhow!("could not acquire lock: {e}"))?;

    if !ClaudeCliBackend::is_available() {
        anyhow::bail!("claude CLI not found on PATH. Required for context audit.");
    }

    let project = git_root_or_cwd()?;

    if verbose {
        eprintln!("[verbose] project path: {}", project);
    }

    println!(
        "{}",
        "Auditing context for redundancy and contradictions (AI-powered)...".cyan()
    );
    println!(
        "  {}",
        "This may take a minute (AI-powered audit)...".dimmed()
    );

    // Snapshot current context
    let snapshot = snapshot_context(&config, &project)?;

    // Build audit prompt
    let skills: Vec<(String, String)> = snapshot
        .skills
        .iter()
        .map(|s| (s.path.clone(), s.content.clone()))
        .collect();
    let agents: Vec<(String, String)> = snapshot
        .global_agents
        .iter()
        .map(|a| (a.path.clone(), a.content.clone()))
        .collect();

    let prompt = prompts::build_audit_prompt(
        snapshot.claude_md.as_deref(),
        &skills,
        snapshot.memory_md.as_deref(),
        &agents,
    );

    let backend = ClaudeCliBackend::new(&config.ai);

    use retro_core::analysis::backend::AnalysisBackend;
    let response = backend.execute(&prompt)?;

    // Parse findings
    let cleaned = strip_code_fences(&response.text);
    let audit_response: AuditResponse = serde_json::from_str(&cleaned).map_err(|e| {
        anyhow::anyhow!(
            "failed to parse audit response: {e}\nraw: {}",
            truncate_str(&response.text, 500)
        )
    })?;

    if audit_response.findings.is_empty() {
        println!("{}", "No issues found — context looks clean!".green());
        return Ok(());
    }

    println!();
    println!(
        "Found {} issues:",
        audit_response.findings.len().to_string().yellow()
    );

    for finding in &audit_response.findings {
        let icon = match finding.finding_type.as_str() {
            "redundant" => "dup",
            "contradictory" => "!!",
            "oversized" => "big",
            "stale" => "old",
            _ => "?",
        };
        println!();
        println!(
            "  {} [{}] {}",
            "!".yellow(),
            icon.dimmed(),
            finding.description.white()
        );
        if !finding.affected_items.is_empty() {
            println!(
                "         {} {}",
                "affects:".dimmed(),
                finding.affected_items.join(", ").dimmed()
            );
        }
        println!(
            "         {} {}",
            "suggestion:".dimmed(),
            finding.suggestion.dimmed()
        );
    }

    if dry_run {
        println!();
        println!(
            "{}",
            "Dry run — showing findings only. Review and address manually."
                .yellow()
                .bold()
        );
    }

    // Audit log
    let audit_details = serde_json::json!({
        "findings_count": audit_response.findings.len(),
        "finding_types": audit_response.findings.iter().map(|f| f.finding_type.as_str()).collect::<Vec<_>>(),
        "cost_usd": response.cost_usd,
        "project": project,
        "dry_run": dry_run,
    });
    audit_log::append(&audit_path, "audit", audit_details)?;

    println!();
    println!(
        "  {} ${:.4}",
        "AI cost:".dimmed(),
        response.cost_usd
    );

    Ok(())
}
