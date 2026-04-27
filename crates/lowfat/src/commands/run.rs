use crate::filters;
use lowfat_core::config::RunfConfig;
use lowfat_core::db::{Db, InvocationRecord, TrackRecord};
use lowfat_core::pipeline::{Pipeline, StageType};
use lowfat_core::tee;
use lowfat_plugin::discovery::{discover_plugins, resolve_plugins, DiscoveredPlugin};
use lowfat_plugin::plugin::{FilterInput, FilterPlugin};
use lowfat_runner::runner::{exec_command, execute_pipeline, HybridRunner};
use std::collections::HashMap;
use std::time::Instant;

/// Main filter path: execute command, apply pipeline, track metrics.
/// Returns the process exit code.
pub fn run(args: &[String]) -> i32 {
    let cmd = &args[0];
    let cmd_args: Vec<String> = args[1..].to_vec();
    let subcommand = cmd_args.first().cloned().unwrap_or_default();

    let config = RunfConfig::resolve();

    if !config.is_enabled(cmd) {
        return passthrough(cmd, &cmd_args, &config);
    }

    // Native built-in filters (git, docker, ls)
    let all_filters = filters::builtins();
    // External plugins from ~/.lowfat/plugins/ (user-installed, runtime-loaded)
    let external_plugins = discover_plugins(&config.plugin_dir);
    let external_map = resolve_plugins(&external_plugins);

    // Execute the real command
    let start = Instant::now();
    let (raw, exit_code) = match exec_command(cmd, &cmd_args) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[lowfat] exec error: {e}");
            return 1;
        }
    };

    // Resolve which filter/pipeline to use
    let filter_name = resolve_filter_name(cmd, &all_filters, &external_plugins, &external_map);
    let pipeline = resolve_pipeline(cmd, exit_code, &raw, &config, &filter_name,
        &external_plugins, &external_map);

    // Build plugin map for pipeline execution: builtins + loaded community plugins
    let mut plugin_map: HashMap<String, Box<dyn FilterPlugin>> = all_filters;
    for stage in &pipeline.stages {
        if stage.stage_type == StageType::Plugin
            && !plugin_map.contains_key(&stage.name)
        {
            if let Some(loaded) = load_external_plugin(&stage.name, &external_plugins, &external_map) {
                plugin_map.insert(stage.name.clone(), loaded);
            }
        }
    }

    let input = FilterInput {
        raw: raw.clone(),
        command: cmd.clone(),
        subcommand: subcommand.clone(),
        args: cmd_args.clone(),
        level: config.level,
        head_limit: config.level.head_limit(40),
        exit_code,
    };

    // Execute the pipeline chain
    let filtered = match execute_pipeline(&pipeline, &raw, &input, &plugin_map) {
        Ok(text) => text,
        Err(_) => raw.clone(),
    };

    let elapsed = start.elapsed().as_millis() as u64;

    // Track
    if let Ok(db) = Db::open(&config.data_dir) {
        let args_str = cmd_args.join(" ");
        let pipeline_desc = pipeline.display();
        let _ = db.track(&TrackRecord {
            original_cmd: format!("{cmd} {args_str}"),
            lowfat_cmd: format!("lowfat[{pipeline_desc}] {cmd} {args_str}"),
            raw: raw.clone(),
            filtered: filtered.clone(),
            exec_time_ms: elapsed,
            project_path: std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
        });
        // Usage history — command+subcommand only, no args. Powers `lowfat history`.
        let known = known_subcommands(cmd, &plugin_map, &external_plugins, &external_map);
        // in_scope = plugin is declared to handle *this* subcommand. Empty
        // `known` means "no subcommand restriction" — treat as universal.
        let in_scope = filter_name.is_some()
            && (known.is_empty() || known.iter().any(|s| s == &subcommand));
        let raw_tokens = lowfat_core::tokens::estimate_tokens(&raw) as u64;
        let filtered_tokens = lowfat_core::tokens::estimate_tokens(&filtered) as u64;
        let _ = db.record_invocation(&InvocationRecord {
            command: cmd.clone(),
            subcommand: history_subcommand(&subcommand, &known),
            raw_tokens,
            filtered_tokens,
            had_plugin: filter_name.is_some(),
            in_scope,
            reduced: filtered_tokens < raw_tokens,
            exit_code,
        });
    }

    // Tee on failure
    let tee_dir = config.data_dir.join("tee");
    tee::save_on_failure(&tee_dir, &format!("{cmd}_{subcommand}"), &raw, exit_code);

    print!("{filtered}");
    exit_code
}

/// Find filter name for a command: native builtins first, then community plugins.
fn resolve_filter_name(
    cmd: &str,
    builtins: &HashMap<String, Box<dyn FilterPlugin>>,
    plugins: &[DiscoveredPlugin],
    cmd_map: &HashMap<String, usize>,
) -> Option<String> {
    // Native builtin? The map key is the command name, filter name is "{cmd}-compact"
    if builtins.contains_key(cmd) {
        return Some(cmd.to_string());
    }
    // Community plugin?
    if let Some(&idx) = cmd_map.get(cmd) {
        return Some(plugins[idx].manifest.plugin.name.clone());
    }
    None
}

/// Resolve which pipeline to use for a command.
fn resolve_pipeline(
    cmd: &str,
    exit_code: i32,
    raw: &str,
    config: &RunfConfig,
    filter_name: &Option<String>,
    plugins: &[DiscoveredPlugin],
    cmd_map: &HashMap<String, usize>,
) -> Pipeline {
    // 1. Check .lowfat conditional pipelines
    if let Some(conditional) = config.pipeline_for(cmd) {
        if let Some(pipeline) = conditional.select(exit_code, raw) {
            return pipeline.clone();
        }
    }

    // 2. Check community plugin init.toml pre/post processors
    if let Some(&idx) = cmd_map.get(cmd) {
        let manifest = &plugins[idx].manifest;
        let name = &manifest.plugin.name;
        if let Some(ref pipe_config) = manifest.pipeline {
            let pre = pipe_config.pre.as_deref().unwrap_or(&[]);
            let post = pipe_config.post.as_deref().unwrap_or(&[]);
            if !pre.is_empty() || !post.is_empty() {
                return Pipeline::from_parts(pre, name, post);
            }
        }
    }

    // 3. Single filter (builtin or community)
    if let Some(ref name) = filter_name {
        return Pipeline::single(name);
    }

    // No filter found
    Pipeline::parse("passthrough")
}

/// Load a community plugin by name.
fn load_external_plugin(
    name: &str,
    plugins: &[DiscoveredPlugin],
    cmd_map: &HashMap<String, usize>,
) -> Option<Box<dyn FilterPlugin>> {
    for plugin in plugins {
        if plugin.manifest.plugin.name == name {
            return HybridRunner::load(plugin).ok();
        }
    }
    if let Some(&idx) = cmd_map.get(name) {
        return HybridRunner::load(&plugins[idx]).ok();
    }
    None
}

/// Normalise the first arg into a subcommand for history.
///
/// Hybrid: if the command has a registered plugin declaring `subcommands`,
/// only accept `first` when it's in that list. Otherwise fall back to a
/// heuristic — a bare identifier is a subcommand, anything else (paths,
/// flags, files with extensions) is not. This keeps `ls /path` grouped
/// under `""` while preserving breakdown for `cargo build` etc.
fn history_subcommand(first: &str, known: &[String]) -> String {
    if first.is_empty() {
        return String::new();
    }
    if !known.is_empty() {
        return if known.iter().any(|s| s == first) {
            first.to_string()
        } else {
            String::new()
        };
    }
    if looks_like_subcommand(first) {
        first.to_string()
    } else {
        String::new()
    }
}

/// Heuristic: a subcommand looks like a bare lowercase identifier.
/// Excludes flags, paths, files with extensions, etc.
fn looks_like_subcommand(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    s.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Known subcommands for `cmd`, pulled from builtin filters or community plugins.
/// Empty means "no registered plugin declares subcommands for this command".
fn known_subcommands(
    cmd: &str,
    builtins: &HashMap<String, Box<dyn FilterPlugin>>,
    plugins: &[DiscoveredPlugin],
    cmd_map: &HashMap<String, usize>,
) -> Vec<String> {
    if let Some(f) = builtins.get(cmd) {
        let subs = f.info().subcommands;
        if !subs.is_empty() {
            return subs;
        }
    }
    if let Some(&idx) = cmd_map.get(cmd) {
        if let Some(ref subs) = plugins[idx].manifest.plugin.subcommands {
            if !subs.is_empty() {
                return subs.clone();
            }
        }
    }
    Vec::new()
}

/// Run command unfiltered, still track as passthrough.
fn passthrough(cmd: &str, args: &[String], config: &RunfConfig) -> i32 {
    let start = Instant::now();
    let (raw, exit_code) = match exec_command(cmd, args) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[lowfat] exec error: {e}");
            return 1;
        }
    };

    let elapsed = start.elapsed().as_millis() as u64;

    if let Ok(db) = Db::open(&config.data_dir) {
        let args_str = args.join(" ");
        let _ = db.track(&TrackRecord {
            original_cmd: format!("{cmd} {args_str}"),
            lowfat_cmd: format!("lowfat:passthrough {cmd} {args_str}"),
            raw: raw.clone(),
            filtered: raw.clone(),
            exec_time_ms: elapsed,
            project_path: std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
        });
        let subcommand = args.first().cloned().unwrap_or_default();
        let tokens = lowfat_core::tokens::estimate_tokens(&raw) as u64;
        let _ = db.record_invocation(&InvocationRecord {
            command: cmd.to_string(),
            subcommand: history_subcommand(&subcommand, &[]),
            raw_tokens: tokens,
            filtered_tokens: tokens,
            had_plugin: false,
            in_scope: false,
            reduced: false,
            exit_code,
        });
    }

    print!("{raw}");
    exit_code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heuristic_accepts_bare_identifier() {
        assert_eq!(history_subcommand("status", &[]), "status");
        assert_eq!(history_subcommand("build", &[]), "build");
        assert_eq!(history_subcommand("pip-compile", &[]), "pip-compile");
    }

    #[test]
    fn heuristic_rejects_paths_and_flags() {
        assert_eq!(history_subcommand("/tmp", &[]), "");
        assert_eq!(history_subcommand("./foo", &[]), "");
        assert_eq!(history_subcommand("-la", &[]), "");
        assert_eq!(history_subcommand("file.rs", &[]), "");
        assert_eq!(history_subcommand("Documents", &[]), "");
        assert_eq!(history_subcommand("", &[]), "");
    }

    #[test]
    fn known_list_overrides_heuristic() {
        let known = vec!["status".to_string(), "log".to_string()];
        assert_eq!(history_subcommand("status", &known), "status");
        // `checkout` is a valid identifier but not in the known list → dropped.
        assert_eq!(history_subcommand("checkout", &known), "");
    }
}
