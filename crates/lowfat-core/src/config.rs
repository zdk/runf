use crate::level::Level;
use crate::pipeline::{ConditionalPipelines, parse_conditional_pipeline};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::PathBuf;

/// Resolved lowfat configuration from env + .lowfat file.
#[derive(Debug)]
pub struct RunfConfig {
    pub level: Level,
    pub disabled: HashSet<String>,
    /// Some = whitelist mode (only these filters active)
    pub allowed: Option<HashSet<String>>,
    pub data_dir: PathBuf,
    pub plugin_dir: PathBuf,
    pub home_dir: PathBuf,
    /// Per-command conditional pipelines from .lowfat config.
    /// Supports: pipeline.git = ..., pipeline.git.error = ..., pipeline.git.large = ...
    /// The special key `pipeline.* = ...` is a global wildcard whose stages
    /// are *prepended* to every resolved pipeline — useful for always-on
    /// processors like `redact-secrets`. See [`pipeline_wildcard`].
    ///
    /// [`pipeline_wildcard`]: RunfConfig::pipeline_wildcard
    pub pipelines: HashMap<String, ConditionalPipelines>,
}

impl RunfConfig {
    /// Resolve configuration from environment and .lowfat config walking.
    pub fn resolve() -> Self {
        let lowfat_home = env::var("LOWFAT_HOME").ok();
        let xdg_config_home = env::var("XDG_CONFIG_HOME").ok();
        let home = dirs_home();
        let home_dir = resolve_home_dir(
            lowfat_home.as_deref(),
            xdg_config_home.as_deref(),
            &home,
            &|p| p.is_dir(),
        );

        let data_dir = env::var("LOWFAT_DATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                env::var("XDG_DATA_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| dirs_home().join(".local/share"))
                    .join("lowfat")
            });

        let plugin_dir = home_dir.join("plugins");

        // Level: LOWFAT_LEVEL env > .lowfat config > default
        let mut level = Level::Full;
        let mut disabled = HashSet::new();
        let mut allowed: Option<HashSet<String>> = None;
        // Collect raw pipeline lines for post-processing into ConditionalPipelines
        // Key: (command, condition) e.g., ("git", "") or ("git", "error")
        let mut pipeline_lines: HashMap<String, Vec<(String, String)>> = HashMap::new();
        let mut pipelines = HashMap::new();

        // Parse .lowfat config (walk up from cwd)
        if let Some(config_path) = find_config() {
            if let Ok(content) = fs::read_to_string(&config_path) {
                for line in content.lines() {
                    let line = line.trim();
                    if line.is_empty() || line.starts_with('#') {
                        continue;
                    }
                    if let Some(val) = line.strip_prefix("level=") {
                        if let Ok(l) = val.parse() {
                            level = l;
                        }
                    } else if let Some(val) = line.strip_prefix("filters=") {
                        allowed = Some(
                            val.split(',').map(|s| s.trim().to_string()).collect(),
                        );
                    } else if let Some(val) = line.strip_prefix("disable=") {
                        for name in val.split(',') {
                            disabled.insert(name.trim().to_string());
                        }
                    } else if let Some(rest) = line.strip_prefix("pipeline.") {
                        // pipeline.git = strip-ansi | git-compact | truncate
                        // pipeline.git.error = strip-ansi | head
                        // pipeline.git.large = git-compact | token-budget
                        // pipeline.* = redact-secrets       (wildcard, prepended)
                        if let Some((key, spec)) = rest.split_once('=') {
                            let key = key.trim();
                            let spec = spec.trim().to_string();
                            // Split "git.error" → cmd="git", condition="error"
                            let (cmd, condition) = match key.split_once('.') {
                                Some((c, cond)) => (c.to_string(), cond.to_string()),
                                None => (key.to_string(), String::new()),
                            };
                            pipeline_lines
                                .entry(cmd)
                                .or_default()
                                .push((condition, spec));
                        }
                    }
                }
            }
        }

        // Build ConditionalPipelines from collected lines
        for (cmd, lines) in pipeline_lines {
            pipelines.insert(cmd, parse_conditional_pipeline(&lines));
        }

        // LOWFAT_DISABLE env overrides
        if let Ok(val) = env::var("LOWFAT_DISABLE") {
            for name in val.split(',') {
                disabled.insert(name.trim().to_string());
            }
        }

        // LOWFAT_LEVEL env takes highest priority
        if let Ok(val) = env::var("LOWFAT_LEVEL") {
            if let Ok(l) = val.parse() {
                level = l;
            }
        }

        // Redaction ruleset: built-in defaults < global redact.conf <
        // project redact.conf (beside the discovered .lowfat).
        let (global_redact, project_redact) =
            crate::redact::paths(&home_dir, find_config().as_deref());
        crate::redact::init(Some(&global_redact), project_redact.as_deref());

        RunfConfig {
            level,
            disabled,
            allowed,
            data_dir,
            plugin_dir,
            home_dir,
            pipelines,
        }
    }

    /// Get the conditional pipelines for a command, if configured.
    pub fn pipeline_for(&self, cmd: &str) -> Option<&ConditionalPipelines> {
        self.pipelines.get(cmd)
    }

    /// Get the wildcard pipeline (`pipeline.* = ...`), if configured.
    /// Callers prepend its stages to whatever pipeline they resolve, so a
    /// rule like `pipeline.* = redact-secrets` applies to every command
    /// without disabling per-command pipelines.
    pub fn pipeline_wildcard(&self) -> Option<&ConditionalPipelines> {
        self.pipelines.get("*")
    }

    /// Check if a filter name is enabled under current config.
    pub fn is_enabled(&self, name: &str) -> bool {
        if self.disabled.contains(name) {
            return false;
        }
        if let Some(ref allowed) = self.allowed {
            return allowed.contains(name);
        }
        true
    }
}

/// Walk up from cwd to find nearest `.lowfat` config file.
pub fn find_config() -> Option<PathBuf> {
    let mut dir = env::current_dir().ok()?;
    loop {
        let candidate = dir.join(".lowfat");
        if candidate.is_file() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn dirs_home() -> PathBuf {
    env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

/// Resolve the plugin / config home directory.
///
/// Precedence (highest to lowest):
///   1. `$LOWFAT_HOME` — explicit override always wins
///   2. `$XDG_CONFIG_HOME/lowfat` if `XDG_CONFIG_HOME` is set
///   3. `~/.config/lowfat` if that directory already exists (XDG default)
///   4. `~/.lowfat` — fallback when none of the above apply
///
/// Home-directory candidates must be directories: a file at
/// `~/.lowfat` is the pipeline config (see [`find_config`]), not a
/// competing home, and is not treated as one here. When both an XDG
/// directory and a legacy `~/.lowfat/` directory exist, prints a
/// one-shot warning to stderr and prefers XDG. Pure function — takes
/// env vars + a `path_is_dir` closure so tests don't touch the real fs.
pub fn resolve_home_dir(
    lowfat_home: Option<&str>,
    xdg_config_home: Option<&str>,
    home: &std::path::Path,
    path_is_dir: &dyn Fn(&std::path::Path) -> bool,
) -> PathBuf {
    if let Some(h) = lowfat_home {
        return PathBuf::from(h);
    }

    let dot_lowfat = home.join(".lowfat");

    if let Some(xdg) = xdg_config_home {
        let xdg_path = PathBuf::from(xdg).join("lowfat");
        warn_if_both_dirs_exist(&xdg_path, &dot_lowfat, path_is_dir);
        return xdg_path;
    }

    let xdg_default = home.join(".config").join("lowfat");
    if path_is_dir(&xdg_default) {
        warn_if_both_dirs_exist(&xdg_default, &dot_lowfat, path_is_dir);
        return xdg_default;
    }

    dot_lowfat
}

fn warn_if_both_dirs_exist(
    chosen: &std::path::Path,
    other: &std::path::Path,
    path_is_dir: &dyn Fn(&std::path::Path) -> bool,
) {
    if chosen != other && path_is_dir(chosen) && path_is_dir(other) {
        eprintln!(
            "[lowfat] warning: both {} and {} exist; using {}. Remove one to silence this.",
            chosen.display(),
            other.display(),
            chosen.display(),
        );
    }
}

/// Find the .lowfat config path (exposed for display purposes).
pub fn find_config_display() -> Option<PathBuf> {
    find_config()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_enabled_default() {
        let config = RunfConfig {
            level: Level::Full,
            disabled: HashSet::new(),
            allowed: None,
            data_dir: PathBuf::new(),
            plugin_dir: PathBuf::new(),
            home_dir: PathBuf::new(),
            pipelines: HashMap::new(),
        };
        assert!(config.is_enabled("git"));
        assert!(config.is_enabled("docker"));
    }

    #[test]
    fn is_enabled_disabled() {
        let mut disabled = HashSet::new();
        disabled.insert("npm".to_string());
        let config = RunfConfig {
            level: Level::Full,
            disabled,
            allowed: None,
            data_dir: PathBuf::new(),
            plugin_dir: PathBuf::new(),
            home_dir: PathBuf::new(),
            pipelines: HashMap::new(),
        };
        assert!(!config.is_enabled("npm"));
        assert!(config.is_enabled("git"));
    }

    #[test]
    fn home_explicit_lowfat_home_wins() {
        let r = resolve_home_dir(
            Some("/custom/lf"),
            Some("/wrong/.config"),
            std::path::Path::new("/home/user"),
            &|_| true,
        );
        assert_eq!(r, PathBuf::from("/custom/lf"));
    }

    #[test]
    fn home_xdg_env_used_when_set_even_if_path_missing() {
        let r = resolve_home_dir(
            None,
            Some("/explicit/.config"),
            std::path::Path::new("/home/user"),
            &|_| false,
        );
        assert_eq!(r, PathBuf::from("/explicit/.config/lowfat"));
    }

    #[test]
    fn home_xdg_default_used_when_path_exists() {
        let home = PathBuf::from("/home/user");
        let xdg = home.join(".config/lowfat");
        let r = resolve_home_dir(None, None, &home, &|p| p == xdg.as_path());
        assert_eq!(r, xdg);
    }

    #[test]
    fn home_dot_lowfat_used_when_neither_xdg_set_nor_exists() {
        let r = resolve_home_dir(
            None,
            None,
            std::path::Path::new("/home/user"),
            &|_| false,
        );
        assert_eq!(r, PathBuf::from("/home/user/.lowfat"));
    }

    #[test]
    fn home_dot_lowfat_used_when_only_it_exists() {
        let home = PathBuf::from("/home/user");
        let dot_lowfat = home.join(".lowfat");
        let r = resolve_home_dir(None, None, &home, &|p| p == dot_lowfat.as_path());
        assert_eq!(r, dot_lowfat);
    }

    #[test]
    fn home_xdg_wins_when_both_exist() {
        let home = PathBuf::from("/home/user");
        let r = resolve_home_dir(None, None, &home, &|_| true);
        assert_eq!(r, home.join(".config/lowfat"));
    }

    // Regression: `~/.lowfat` as a *file* is the pipeline config, not a
    // competing home directory. Resolution must pick XDG without warning,
    // and must not treat the file as a usable home.
    #[test]
    fn home_xdg_used_when_dot_lowfat_is_file_only() {
        let home = PathBuf::from("/home/user");
        let xdg_dir = home.join(".config/lowfat");
        // path_is_dir reports only the XDG path as a directory; ~/.lowfat
        // exists on the real fs as a file but is_dir returns false.
        let r = resolve_home_dir(
            None,
            Some("/home/user/.config"),
            &home,
            &|p| p == xdg_dir.as_path(),
        );
        assert_eq!(r, xdg_dir);
    }

    #[test]
    fn pipeline_wildcard_resolves() {
        let mut pipelines = HashMap::new();
        pipelines.insert(
            "*".to_string(),
            parse_conditional_pipeline(&[("".into(), "redact-secrets".into())]),
        );
        let config = RunfConfig {
            level: Level::Full,
            disabled: HashSet::new(),
            allowed: None,
            data_dir: PathBuf::new(),
            plugin_dir: PathBuf::new(),
            home_dir: PathBuf::new(),
            pipelines,
        };
        assert!(config.pipeline_wildcard().is_some());
        // pipeline_for is unchanged: exact-match only, no fallback.
        assert!(config.pipeline_for("anything").is_none());
    }

    #[test]
    fn pipeline_wildcard_absent_by_default() {
        let config = RunfConfig {
            level: Level::Full,
            disabled: HashSet::new(),
            allowed: None,
            data_dir: PathBuf::new(),
            plugin_dir: PathBuf::new(),
            home_dir: PathBuf::new(),
            pipelines: HashMap::new(),
        };
        assert!(config.pipeline_wildcard().is_none());
    }

    #[test]
    fn is_enabled_whitelist() {
        let mut allowed = HashSet::new();
        allowed.insert("git".to_string());
        allowed.insert("docker".to_string());
        let config = RunfConfig {
            level: Level::Full,
            disabled: HashSet::new(),
            allowed: Some(allowed),
            data_dir: PathBuf::new(),
            plugin_dir: PathBuf::new(),
            home_dir: PathBuf::new(),
            pipelines: HashMap::new(),
        };
        assert!(config.is_enabled("git"));
        assert!(!config.is_enabled("npm"));
    }
}
