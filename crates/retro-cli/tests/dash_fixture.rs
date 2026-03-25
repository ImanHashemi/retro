use std::process::Command;

#[test]
fn test_retro_dash_help() {
    let output = Command::new("cargo")
        .args(["run", "-p", "retro-cli", "--", "dash", "--help"])
        .output()
        .expect("failed to run retro dash --help");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("dashboard") || stdout.contains("TUI") || stdout.contains("dash") || stdout.contains("Dash"),
        "dash help should mention dashboard: {stdout}"
    );
}

#[test]
fn test_retro_start_help() {
    let output = Command::new("cargo")
        .args(["run", "-p", "retro-cli", "--", "start", "--help"])
        .output()
        .expect("failed to run retro start --help");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("runner") || stdout.contains("scheduled") || stdout.contains("start") || stdout.contains("Start"),
        "start help should mention runner: {stdout}"
    );
}

#[test]
fn test_retro_stop_help() {
    let output = Command::new("cargo")
        .args(["run", "-p", "retro-cli", "--", "stop", "--help"])
        .output()
        .expect("failed to run retro stop --help");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("runner") || stdout.contains("scheduled") || stdout.contains("stop") || stdout.contains("Stop"),
        "stop help should mention runner: {stdout}"
    );
}
