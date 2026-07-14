use anyhow::Result;
use retro_core::config::{Config, retro_dir};

/// Start the dashboard server and open the browser. Blocks until Ctrl+C.
pub fn run(no_open: bool) -> Result<()> {
    let dir = retro_dir();
    let config = Config::load(&dir.join("config.toml"))?;
    if !config.v3.enabled {
        anyhow::bail!("v3 is disabled — run `retro init --v3` first");
    }
    let url = format!("http://127.0.0.1:{}", config.ui.port);
    if !no_open {
        // macOS `open`; failure is non-fatal (headless/SSH)
        let _ = std::process::Command::new("open").arg(&url).spawn();
    }
    crate::ui::serve(dir, config)
}
