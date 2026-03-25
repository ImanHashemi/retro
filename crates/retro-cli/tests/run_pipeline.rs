use std::path::PathBuf;
use std::process::Command;

fn retro_bin() -> PathBuf {
    // CARGO_MANIFEST_DIR is retro-cli crate root; workspace root is two levels up
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let workspace_root = PathBuf::from(manifest_dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    workspace_root.join("target/debug/retro")
}

#[test]
fn test_retro_run_dry_run() {
    let output = Command::new(retro_bin())
        .args(["run", "--dry-run"])
        .output()
        .expect("failed to execute retro run");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Should not panic — either succeeds or says "not initialized"
    assert!(
        output.status.success() || stderr.contains("not initialized") || stdout.contains("not initialized"),
        "retro run --dry-run failed unexpectedly: stdout={stdout}, stderr={stderr}"
    );
}

#[test]
fn test_retro_run_help() {
    let output = Command::new(retro_bin())
        .args(["run", "--help"])
        .output()
        .expect("failed to execute retro run --help");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("pipeline") || stdout.contains("Run the full"));
    assert!(output.status.success());
}
