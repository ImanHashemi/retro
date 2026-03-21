use anyhow::{Context, Result};
use retro_core::config::{retro_dir, Config};
use std::path::{Path, PathBuf};
use std::process::Command;

const PLIST_LABEL: &str = "com.retro.runner";

pub fn plist_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home)
        .join("Library/LaunchAgents")
        .join(format!("{PLIST_LABEL}.plist"))
}

pub fn generate_plist(config: &Config) -> Result<String> {
    let binary = std::env::current_exe()
        .context("resolving retro binary path")?
        .to_string_lossy()
        .to_string();
    let retro_dir = retro_dir();
    let log_path = retro_dir.join("runner.log");

    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{PLIST_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{binary}</string>
        <string>run</string>
    </array>
    <key>StartInterval</key>
    <integer>{interval}</integer>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>
"#,
        interval = config.runner.interval_seconds,
        log = log_path.display(),
    ))
}

fn get_uid() -> u32 {
    unsafe { libc::getuid() }
}

pub fn is_loaded() -> bool {
    let uid = get_uid();
    Command::new("launchctl")
        .args(["print", &format!("gui/{uid}/{PLIST_LABEL}")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn load(plist_path: &Path) -> Result<()> {
    let uid = get_uid();
    let output = Command::new("launchctl")
        .args(["bootstrap", &format!("gui/{uid}"), &plist_path.to_string_lossy()])
        .output()
        .context("running launchctl bootstrap")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("launchctl bootstrap failed: {stderr}");
    }
    Ok(())
}

pub fn unload() -> Result<()> {
    let uid = get_uid();
    let output = Command::new("launchctl")
        .args(["bootout", &format!("gui/{uid}/{PLIST_LABEL}")])
        .output()
        .context("running launchctl bootout")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.contains("No such process") && !stderr.contains("Could not find service") {
            anyhow::bail!("launchctl bootout failed: {stderr}");
        }
    }
    Ok(())
}

pub fn install_and_load(config: &Config) -> Result<()> {
    let path = plist_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("creating LaunchAgents directory")?;
    }
    let xml = generate_plist(config)?;
    std::fs::write(&path, &xml).context("writing plist file")?;
    if is_loaded() {
        let _ = unload();
    }
    load(&path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plist_path() {
        let path = plist_path();
        assert!(path.to_str().unwrap().contains("Library/LaunchAgents"));
        assert!(path.to_str().unwrap().ends_with("com.retro.runner.plist"));
    }

    #[test]
    fn test_generate_plist_contains_required_keys() {
        let config = Config::default();
        let xml = generate_plist(&config).unwrap();
        assert!(xml.contains("<key>Label</key>"));
        assert!(xml.contains(&format!("<string>{PLIST_LABEL}</string>")));
        assert!(xml.contains("<key>StartInterval</key>"));
        assert!(xml.contains("<key>RunAtLoad</key>"));
        assert!(xml.contains("<key>StandardOutPath</key>"));
        assert!(xml.contains("<key>StandardErrorPath</key>"));
        assert!(xml.contains("runner.log"));
    }

    #[test]
    fn test_generate_plist_uses_config_interval() {
        let mut config = Config::default();
        config.runner.interval_seconds = 600;
        let xml = generate_plist(&config).unwrap();
        assert!(xml.contains("<integer>600</integer>"));
    }

    #[test]
    fn test_generate_plist_valid_xml() {
        let config = Config::default();
        let xml = generate_plist(&config).unwrap();
        assert!(xml.starts_with("<?xml version="));
        assert!(xml.contains("<!DOCTYPE plist"));
        assert!(xml.contains("<plist version=\"1.0\">"));
        assert!(xml.ends_with("</plist>\n"));
    }

    #[test]
    fn test_get_uid_returns() {
        let uid = get_uid();
        let _ = uid; // Just check it doesn't panic
    }
}
