use anyhow::Result;

/// Canonical command-rewrite logic — the single source of truth shared by the
/// Claude Code hook and the OpenCode plugin.
///
/// Returns `Some("lowfat <command>")` when a filter (builtin, plugin, or
/// configured pipeline) applies, otherwise `None` (pass through unchanged).
pub fn rewrite_command(command: &str) -> Option<String> {
    let base_cmd = command.split_whitespace().next().unwrap_or("");

    // Skip empty input or anything already routed through lowfat.
    if base_cmd.is_empty() || base_cmd == "lowfat" || base_cmd == "lf" {
        return None;
    }

    let config = lowfat_core::config::RunfConfig::resolve();
    if !config.is_enabled(base_cmd) {
        return None;
    }

    // A wildcard pipeline means every command must route through lowfat so the
    // prepended stages (e.g. redact-secrets) fire.
    let builtins = crate::filters::builtins();
    let plugins = lowfat_plugin::discovery::discover_plugins(&config.plugin_dir);
    let plugin_map = lowfat_plugin::discovery::resolve_plugins(&plugins);
    let has_filter = builtins.contains_key(base_cmd)
        || plugin_map.contains_key(base_cmd)
        || config.pipeline_for(base_cmd).is_some()
        || config.pipeline_wildcard().is_some();

    if !has_filter {
        return None;
    }

    Some(format!("lowfat {command}"))
}

/// `lowfat rewrite <command...>` — print the rewritten command on stdout.
///
/// Agent plugins call this and swap in the result. If no filter applies the
/// original command is echoed back unchanged (so callers can diff and no-op).
pub fn run(args: &[String]) -> Result<()> {
    let command = args.join(" ");
    let command = command.trim();

    match rewrite_command(command) {
        Some(rewritten) => println!("{rewritten}"),
        None => println!("{command}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_already_wrapped() {
        assert_eq!(rewrite_command("lowfat git status"), None);
        assert_eq!(rewrite_command("lf git status"), None);
    }

    #[test]
    fn skips_empty() {
        assert_eq!(rewrite_command(""), None);
    }

    #[test]
    fn skips_unknown_command() {
        // curl has no builtin/plugin filter by default.
        assert_eq!(rewrite_command("curl https://example.com"), None);
    }

    #[test]
    fn wraps_known_command() {
        // git ships as an embedded plugin.
        assert_eq!(
            rewrite_command("git status"),
            Some("lowfat git status".into())
        );
    }
}
