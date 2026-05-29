use anyhow::{Context, Result};
use std::env;
use std::fs;
use std::path::PathBuf;

// Plugin source baked into the binary so install needs no network or repo.
// Must live inside the crate (not the workspace root) so `cargo publish` ships
// it — see crates/lowfat-plugin/src/embedded.rs for the same constraint.
const PLUGIN_TS: &str = include_str!("../../embedded/opencode/lowfat.ts");

/// Resolve `~/.config/opencode/plugins/lowfat.ts`, honoring `$XDG_CONFIG_HOME`.
fn plugin_path() -> Result<PathBuf> {
    // Treat an empty $XDG_CONFIG_HOME as unset (fall back to ~/.config).
    let config_home = match env::var("XDG_CONFIG_HOME").ok().filter(|s| !s.is_empty()) {
        Some(xdg) => PathBuf::from(xdg),
        None => home_dir()
            .context("cannot resolve config home (set $HOME or $XDG_CONFIG_HOME)")?
            .join(".config"),
    };
    Ok(config_home.join("opencode").join("plugins").join("lowfat.ts"))
}

fn home_dir() -> Option<PathBuf> {
    env::var("HOME")
        .or_else(|_| env::var("USERPROFILE")) // Windows
        .ok()
        .map(PathBuf::from)
}

/// `lowfat opencode install` — write the plugin into OpenCode's global dir.
pub fn install() -> Result<()> {
    let path = plugin_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(&path, PLUGIN_TS).with_context(|| format!("write {}", path.display()))?;

    println!("✓ Installed lowfat OpenCode plugin → {}", path.display());
    println!("  Restart OpenCode, then run any command (e.g. `git status`).");
    Ok(())
}

/// `lowfat opencode uninstall` — remove the plugin file.
pub fn uninstall() -> Result<()> {
    let path = plugin_path()?;
    if path.exists() {
        fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
        println!("✓ Removed lowfat OpenCode plugin: {}", path.display());
    } else {
        println!("lowfat OpenCode plugin not installed (nothing to remove).");
    }
    Ok(())
}
