use anyhow::Result;
use lowfat_core::pipeline::{apply_builtin, proc_normalize, Pipeline, StageType};
use lowfat_plugin::discovery::{DiscoveredPlugin, PluginSource};
use lowfat_plugin::plugin::{FilterInput, FilterPlugin, PluginInfo};
use lowfat_plugin::security;
use std::collections::HashMap;

use crate::lf_filter::LfFilter;
use crate::process::ProcessFilter;

/// Loads a discovered plugin into a runnable [`FilterPlugin`]. Dispatches by
/// source first, then by entry extension:
/// - [`PluginSource::Embedded`] → always [`LfFilter`] from the in-memory string
/// - [`PluginSource::Disk`] with `.lf` entry → [`LfFilter`] reading from disk
/// - [`PluginSource::Disk`] with any other extension → [`ProcessFilter`] via `sh`
///
/// The entrypoint for disk plugins is resolved by `RuntimeConfig::resolve_entry`:
/// an explicit `runtime.entry` in the manifest wins; otherwise auto-detected.
pub struct HybridRunner;

impl HybridRunner {
    pub fn load(plugin: &DiscoveredPlugin) -> Result<Box<dyn FilterPlugin>> {
        let manifest = &plugin.manifest;
        let info = PluginInfo {
            name: manifest.plugin.name.clone(),
            version: manifest
                .plugin
                .version
                .clone()
                .unwrap_or_else(|| "0.0.0".to_string()),
            commands: manifest.plugin.commands.clone(),
            subcommands: manifest
                .plugin
                .subcommands
                .clone()
                .unwrap_or_default(),
        };

        match &plugin.source {
            PluginSource::Embedded { filter_lf } => {
                let entry = plugin.base_dir().join("filter.lf");
                let filter = LfFilter::from_source(info, filter_lf, entry)?;
                Ok(Box::new(filter))
            }
            PluginSource::Disk { base_dir } => {
                let entry_path = base_dir.join(manifest.runtime.resolve_entry(base_dir));

                if let Err(e) = security::validate_plugin(manifest, base_dir) {
                    anyhow::bail!("security check failed for '{}': {e}", manifest.plugin.name);
                }

                let is_lf = entry_path
                    .extension()
                    .map(|e| e == "lf")
                    .unwrap_or(false);
                if is_lf {
                    let filter = LfFilter::load(info, entry_path)?;
                    Ok(Box::new(filter))
                } else {
                    let filter = ProcessFilter {
                        info,
                        entry: entry_path,
                        base_dir: base_dir.clone(),
                    };
                    Ok(Box::new(filter))
                }
            }
        }
    }
}

/// Execute a pipeline chain against raw command output.
/// Chains built-in processors and plugin filters in order.
///
/// For built-in stages: runs in-process (zero overhead).
/// For plugin stages: looks up the plugin by name and delegates.
pub fn execute_pipeline(
    pipeline: &Pipeline,
    raw: &str,
    input_template: &FilterInput,
    plugin_map: &HashMap<String, Box<dyn FilterPlugin>>,
) -> Result<String> {
    let mut text = raw.to_string();

    for stage in &pipeline.stages {
        // Plugin override: if a plugin exists with the same name as a builtin, plugin wins.
        // This lets users replace any built-in processor with their own implementation.
        if let Some(filter) = plugin_map.get(&stage.name) {
            let mut stage_input = input_template.clone();
            stage_input.raw = text.clone();
            match filter.filter(&stage_input) {
                Ok(out) if !out.passthrough => {
                    text = out.text;
                }
                Ok(_) => {}
                Err(_) => {}
            }
            continue;
        }

        // Fall back to built-in processor
        if stage.stage_type == StageType::Builtin {
            if let Some(processed) = apply_builtin(&stage.name, &text, input_template.level, stage.param, stage.pattern.as_deref()) {
                text = processed;
            }
        }
        // Unknown plugin not in map → skip (passthrough)
    }

    // Final cleanup: trim trailing whitespace, collapse blank lines
    Ok(proc_normalize(&text))
}

/// Execute a command and capture its output.
pub fn exec_command(cmd: &str, args: &[String]) -> Result<(String, i32)> {
    let output = std::process::Command::new(cmd)
        .args(args)
        .output()?;

    let exit_code = output.status.code().unwrap_or(1);
    let mut combined = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.is_empty() {
        if !combined.is_empty() {
            combined.push('\n');
        }
        combined.push_str(&stderr);
    }

    Ok((combined, exit_code))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lowfat_core::level::Level;
    use lowfat_core::pipeline::Pipeline;

    fn make_input(raw: &str) -> FilterInput {
        FilterInput {
            raw: raw.to_string(),
            command: "test".to_string(),
            subcommand: String::new(),
            args: vec![],
            level: Level::Full,
            head_limit: 40,
            exit_code: 0,
        }
    }

    #[test]
    fn execute_builtin_only_pipeline() {
        let pipeline = Pipeline::parse("strip-ansi | dedup-blank");
        let raw = "\x1b[31mERROR\x1b[0m\n\n\n\nline2";
        let input = make_input(raw);
        let result = execute_pipeline(&pipeline, raw, &input, &HashMap::new()).unwrap();
        assert_eq!(result, "ERROR\n\nline2\n");  // normalize collapses blanks + trims
    }

    #[test]
    fn execute_passthrough_pipeline() {
        let pipeline = Pipeline::parse("passthrough");
        let raw = "hello world";
        let input = make_input(raw);
        let result = execute_pipeline(&pipeline, raw, &input, &HashMap::new()).unwrap();
        assert_eq!(result, "hello world\n");  // normalize ensures trailing newline
    }

    #[test]
    fn execute_truncate_pipeline() {
        let pipeline = Pipeline::parse("head");
        let raw = (0..100).map(|i| format!("line{i}")).collect::<Vec<_>>().join("\n");
        let input = make_input(&raw);
        let result = execute_pipeline(&pipeline, &raw, &input, &HashMap::new()).unwrap();
        // Full level head limit for base 40 = 40 lines
        let line_count = result.lines().count();
        assert!(line_count <= 41); // 40 lines + truncation message
    }

    #[test]
    fn execute_chain_strip_then_truncate() {
        let pipeline = Pipeline::parse("strip-ansi | head");
        let mut raw = String::new();
        for i in 0..100 {
            raw.push_str(&format!("\x1b[32mline{i}\x1b[0m\n"));
        }
        let input = make_input(&raw);
        let result = execute_pipeline(&pipeline, &raw, &input, &HashMap::new()).unwrap();
        // Should be ANSI-stripped AND truncated
        assert!(!result.contains("\x1b["));
        assert!(result.lines().count() <= 41);
    }

    #[test]
    fn missing_plugin_skipped() {
        let pipeline = Pipeline::parse("strip-ansi | nonexistent-plugin | head");
        let raw = "\x1b[31mhello\x1b[0m\nworld";
        let input = make_input(raw);
        // nonexistent-plugin is StageType::Plugin, not in map → skipped
        let result = execute_pipeline(&pipeline, raw, &input, &HashMap::new()).unwrap();
        assert!(result.contains("hello"));
        assert!(!result.contains("\x1b["));
    }
}
