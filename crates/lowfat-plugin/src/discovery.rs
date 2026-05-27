use crate::embedded::{EmbeddedPlugin, EMBEDDED};
use crate::manifest::PluginManifest;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Where a plugin's `filter.lf` lives. Disk-backed plugins are user-installed
/// under `~/.lowfat/plugins/`; embedded plugins are baked into the binary at
/// compile time (see [`crate::embedded`]).
#[derive(Debug)]
pub enum PluginSource {
    Disk { base_dir: PathBuf },
    Embedded { filter_lf: &'static str },
}

impl PluginSource {
    /// Path to use for resolving relative paths, logs, etc. Embedded plugins
    /// get a synthetic `<embedded>/<category>/<name>` placeholder so callers
    /// that just want to print a location have something readable.
    pub fn display_path(&self, category: &str, name: &str) -> PathBuf {
        match self {
            PluginSource::Disk { base_dir } => base_dir.clone(),
            PluginSource::Embedded { .. } => {
                PathBuf::from(format!("<embedded>/{category}/{name}"))
            }
        }
    }

    pub fn is_embedded(&self) -> bool {
        matches!(self, PluginSource::Embedded { .. })
    }
}

/// A discovered plugin with its manifest and source.
#[derive(Debug)]
pub struct DiscoveredPlugin {
    pub manifest: PluginManifest,
    pub category: String,
    pub source: PluginSource,
}

impl DiscoveredPlugin {
    /// Path to the plugin's root directory (for disk-backed plugins) or a
    /// synthetic `<embedded>/...` placeholder. Use for display only.
    pub fn base_dir(&self) -> PathBuf {
        self.source.display_path(&self.category, &self.manifest.plugin.name)
    }

    pub fn is_embedded(&self) -> bool {
        self.source.is_embedded()
    }
}

/// Discover plugins from `plugin_dir` (`~/.lowfat/plugins/` by default) merged
/// with the embedded set baked into the binary. Disk plugins win on name
/// collision — a user-installed `git-compact` shadows the bundled one.
///
/// Directory structure for disk plugins:
///   plugin_dir/category/plugin-name/lowfat.toml (or init.toml)
pub fn discover_plugins(plugin_dir: &Path) -> Vec<DiscoveredPlugin> {
    let mut plugins = Vec::new();
    scan_plugin_dir(plugin_dir, &mut plugins);

    // Append embedded plugins whose names aren't already taken by a disk
    // plugin. Disk wins so the user can override a bundled `git-compact`
    // by writing one to ~/.lowfat/plugins/git/git-compact/.
    let taken: std::collections::HashSet<String> = plugins
        .iter()
        .map(|p| p.manifest.plugin.name.clone())
        .collect();
    for emb in EMBEDDED {
        if taken.contains(emb.name) {
            continue;
        }
        if let Some(plugin) = build_embedded(emb) {
            plugins.push(plugin);
        }
    }
    plugins
}

fn build_embedded(emb: &'static EmbeddedPlugin) -> Option<DiscoveredPlugin> {
    let manifest = match PluginManifest::parse(emb.manifest) {
        Ok(m) => m,
        Err(e) => {
            eprintln!(
                "[lowfat] internal: embedded plugin {} has invalid manifest: {e}",
                emb.name
            );
            return None;
        }
    };
    Some(DiscoveredPlugin {
        manifest,
        category: emb.category.into(),
        source: PluginSource::Embedded {
            filter_lf: emb.filter_lf,
        },
    })
}

fn scan_plugin_dir(dir: &Path, plugins: &mut Vec<DiscoveredPlugin>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for category_entry in entries.flatten() {
        let category_path = category_entry.path();
        if !category_path.is_dir() {
            continue;
        }
        let category = category_entry
            .file_name()
            .to_string_lossy()
            .to_string();

        let plugin_entries = match fs::read_dir(&category_path) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for plugin_entry in plugin_entries.flatten() {
            let plugin_path = plugin_entry.path();

            // Try lowfat.toml first, then init.toml for backwards compat
            let manifest_path = if plugin_path.join("lowfat.toml").is_file() {
                plugin_path.join("lowfat.toml")
            } else if plugin_path.join("init.toml").is_file() {
                plugin_path.join("init.toml")
            } else {
                continue;
            };

            let content = match fs::read_to_string(&manifest_path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let manifest = match PluginManifest::parse(&content) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!(
                        "[lowfat] warning: invalid manifest at {}: {}",
                        manifest_path.display(),
                        e
                    );
                    continue;
                }
            };

            plugins.push(DiscoveredPlugin {
                manifest,
                category: category.clone(),
                source: PluginSource::Disk { base_dir: plugin_path },
            });
            break;
        }
    }
}

/// Build a command → plugin mapping. If multiple plugins claim the same command,
/// the last one wins.
pub fn resolve_plugins(plugins: &[DiscoveredPlugin]) -> HashMap<String, usize> {
    let mut map = HashMap::new();
    for (idx, plugin) in plugins.iter().enumerate() {
        for cmd in &plugin.manifest.plugin.commands {
            map.insert(cmd.clone(), idx);
        }
    }
    map
}
