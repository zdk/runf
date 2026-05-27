use anyhow::Result;
use lowfat_core::config::RunfConfig;
use lowfat_plugin::discovery::{discover_plugins, DiscoveredPlugin};
use std::fmt::Write as _;

pub fn run(commands_only: bool) -> Result<()> {
    let config = RunfConfig::resolve();
    let plugins = discover_plugins(&config.plugin_dir);

    if commands_only {
        // Print one command per line — used by shell-init
        let cmds = collect_commands(&config, &plugins);
        for cmd in cmds {
            println!("{cmd}");
        }
        return Ok(());
    }

    let output = format_filters(&config, &plugins);
    print!("{output}");
    Ok(())
}

/// Collect all wrappable command names from plugins (disk + embedded) and
/// pipeline declarations in `.lowfat`. Used by `shell-init` to know which
/// commands to wrap with the lowfat shim.
fn collect_commands(config: &RunfConfig, plugins: &[DiscoveredPlugin]) -> Vec<String> {
    use std::collections::BTreeSet;

    let mut cmds = BTreeSet::new();
    for plugin in plugins {
        for cmd in &plugin.manifest.plugin.commands {
            cmds.insert(cmd.clone());
        }
    }
    for cmd in config.pipelines.keys() {
        cmds.insert(cmd.clone());
    }
    cmds.into_iter().filter(|c| config.is_enabled(c)).collect()
}

/// Format filter listing — testable without side effects.
fn format_filters(config: &RunfConfig, plugins: &[DiscoveredPlugin]) -> String {
    let mut out = String::new();

    writeln!(out, "Filters (lowfat):").unwrap();
    if let Some(cfg_path) = lowfat_core::config::find_config_display() {
        writeln!(out, "  config: {}", cfg_path.display()).unwrap();
    }
    writeln!(out, "  level: {}", config.level).unwrap();
    writeln!(out).unwrap();

    if plugins.is_empty() {
        writeln!(out, "  (no plugins found)").unwrap();
        return out;
    }

    // Split bundled (embedded into the binary) from disk-installed for display.
    let (bundled, community): (Vec<_>, Vec<_>) = plugins
        .iter()
        .partition(|p| p.is_embedded());

    if !bundled.is_empty() {
        writeln!(out, "  bundled:").unwrap();
        for plugin in &bundled {
            let name = &plugin.manifest.plugin.name;
            let cmds = plugin.manifest.plugin.commands.join(", ");
            let enabled = plugin
                .manifest
                .plugin
                .commands
                .iter()
                .all(|c| config.is_enabled(c));
            format_filter(&mut out, name, &cmds, enabled);
        }
    }

    if !community.is_empty() {
        if !bundled.is_empty() {
            writeln!(out).unwrap();
        }
        writeln!(out, "  community:").unwrap();
        for plugin in &community {
            let name = &plugin.manifest.plugin.name;
            let cmds = plugin.manifest.plugin.commands.join(", ");
            let enabled = plugin
                .manifest
                .plugin
                .commands
                .iter()
                .all(|c| config.is_enabled(c));
            format_filter(&mut out, name, &cmds, enabled);
        }
    }

    out
}

fn format_filter(out: &mut String, name: &str, cmds: &str, enabled: bool) {
    if enabled {
        writeln!(out, "  \x1b[92m●\x1b[0m {name}  {cmds}").unwrap();
    } else {
        writeln!(out, "  \x1b[2m○ {name}  {cmds}\x1b[0m").unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lowfat_core::config::RunfConfig;
    use lowfat_core::level::Level;
    use std::collections::{HashMap, HashSet};
    use std::path::PathBuf;

    fn default_config() -> RunfConfig {
        RunfConfig {
            level: Level::Full,
            disabled: HashSet::new(),
            allowed: None,
            data_dir: PathBuf::new(),
            plugin_dir: PathBuf::new(),
            home_dir: PathBuf::new(),
            pipelines: HashMap::new(),
        }
    }

    fn config_with_disabled(disabled: &[&str]) -> RunfConfig {
        let mut config = default_config();
        for d in disabled {
            config.disabled.insert(d.to_string());
        }
        config
    }

    /// Render `format_filters` against the embedded plugins that
    /// `discover_plugins(empty_dir)` yields (git/docker/ls fall back to the
    /// bundled set when nothing's on disk).
    fn render_with_bundled(config: &RunfConfig) -> String {
        let plugins = lowfat_plugin::discovery::discover_plugins(std::path::Path::new("/nonexistent-dir-for-test"));
        format_filters(config, &plugins)
    }

    #[test]
    fn shows_bundled_plugins() {
        let config = default_config();
        let output = render_with_bundled(&config);
        assert!(output.contains("bundled:"));
        assert!(output.contains("git-compact"));
        assert!(output.contains("docker-compact"));
        assert!(output.contains("ls-compact"));
    }

    #[test]
    fn disabled_filter_uses_dim_marker() {
        let config = config_with_disabled(&["git"]);
        let output = render_with_bundled(&config);
        for line in output.lines() {
            if line.contains("git-compact") {
                assert!(line.contains("○"), "disabled filter should use ○ marker");
                return;
            }
        }
        panic!("git-compact not found in output");
    }

    #[test]
    fn enabled_filter_uses_green_marker() {
        let config = default_config();
        let output = render_with_bundled(&config);
        for line in output.lines() {
            if line.contains("git-compact") {
                assert!(line.contains("●"), "enabled filter should use ● marker");
                return;
            }
        }
        panic!("git-compact not found in output");
    }

    #[test]
    fn no_duplicate_entries() {
        let config = default_config();
        let output = render_with_bundled(&config);
        let git_count = output.matches("git-compact").count();
        assert_eq!(git_count, 1, "git-compact should appear once, got {git_count}");
    }

    #[test]
    fn shows_level() {
        let config = default_config();
        let output = format_filters(&config, &[]);
        assert!(output.contains("level: full"));
    }
}
