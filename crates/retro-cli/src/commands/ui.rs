use anyhow::Result;
use retro_core::config::{Config, retro_dir};

/// Start the dashboard server and open the browser. Blocks until Ctrl+C.
pub fn run(no_open: bool) -> Result<()> {
    let dir = retro_dir();
    if !dir.join("knowledge").exists() {
        anyhow::bail!("retro is not initialized — run `retro init`");
    }
    let config = Config::load(&dir.join("config.toml"))?;
    let url = format!("http://127.0.0.1:{}", config.ui.port);
    if !no_open {
        // macOS `open`; failure is non-fatal (headless/SSH)
        let _ = std::process::Command::new("open").arg(&url).spawn();
    }
    crate::ui::serve(dir, config)
}
