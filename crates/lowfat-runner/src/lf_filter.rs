use anyhow::{Context, Result};
use lowfat_core::lf::{self, ExecCtx};
use lowfat_plugin::plugin::{FilterInput, FilterOutput, FilterPlugin, PluginInfo};
use std::path::PathBuf;

/// Runs a `.lf` plugin in-process by executing the parsed [`lf::RuleSet`].
/// Shell and Python escape hatches still spawn subprocesses, but built-in
/// ops (keep/drop/head/tail/else) run without forking.
pub struct LfFilter {
    pub info: PluginInfo,
    pub ruleset: lf::RuleSet,
    pub entry: PathBuf,
}

impl LfFilter {
    pub fn load(info: PluginInfo, entry: PathBuf) -> Result<Self> {
        let source = std::fs::read_to_string(&entry)
            .with_context(|| format!("reading {}", entry.display()))?;
        let ruleset =
            lf::parse(&source).with_context(|| format!("parsing {}", entry.display()))?;
        Ok(Self {
            info,
            ruleset,
            entry,
        })
    }

    /// Build from an in-memory `.lf` source — used by embedded plugins where
    /// the source string lives in `.rodata` and never touches disk. `entry`
    /// is a synthetic display-only path for error messages.
    pub fn from_source(info: PluginInfo, source: &str, entry: PathBuf) -> Result<Self> {
        let ruleset =
            lf::parse(source).with_context(|| format!("parsing {}", entry.display()))?;
        Ok(Self {
            info,
            ruleset,
            entry,
        })
    }
}

impl FilterPlugin for LfFilter {
    fn info(&self) -> PluginInfo {
        self.info.clone()
    }

    fn filter(&self, input: &FilterInput) -> Result<FilterOutput> {
        let ctx = ExecCtx {
            sub: &input.subcommand,
            level: input.level,
            exit_code: input.exit_code,
            args: &input.args,
        };
        // On execution error, degrade to passthrough — never make output
        // worse than no filter at all.
        match lf::execute(&self.ruleset, &ctx, &input.raw) {
            Ok(text) => Ok(FilterOutput {
                passthrough: text.is_empty(),
                text,
            }),
            Err(e) => {
                eprintln!("[lowfat] {} filter error: {e:#}", self.info.name);
                Ok(FilterOutput {
                    passthrough: true,
                    text: input.raw.clone(),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lowfat_core::level::Level;
    use std::io::Write;

    fn make_input(raw: &str, sub: &str, level: Level) -> FilterInput {
        FilterInput {
            raw: raw.to_string(),
            command: "test".into(),
            subcommand: sub.into(),
            args: vec![],
            level,
            head_limit: 30,
            exit_code: 0,
        }
    }

    fn write_lf(name: &str, body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lowfat-lf-test-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("filter.lf");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        path
    }

    fn info() -> PluginInfo {
        PluginInfo {
            name: "test".into(),
            version: "0.0.0".into(),
            commands: vec!["test".into()],
            subcommands: vec![],
        }
    }

    #[test]
    fn lf_filter_runs_keep_head() {
        let path = write_lf(
            "kh",
            r#"
status:
    keep /^M /
    head 2
"#,
        );
        let f = LfFilter::load(info(), path).unwrap();
        let out = f
            .filter(&make_input(
                "M one\n?? two\nM three\nM four\nM five\n",
                "status",
                Level::Full,
            ))
            .unwrap();
        assert_eq!(out.text, "M one\nM three\n");
    }

    #[test]
    fn lf_filter_passthrough_on_parse_error_falls_back() {
        // Write a deliberately broken .lf file
        let path = write_lf("bad", "this is not valid syntax @!#\n");
        let res = LfFilter::load(info(), path);
        assert!(res.is_err(), "expected parse error");
    }

    #[test]
    fn lf_filter_no_match_passes_through() {
        let path = write_lf(
            "nm",
            r#"
specific:
    head 1
"#,
        );
        let f = LfFilter::load(info(), path).unwrap();
        let out = f
            .filter(&make_input("a\nb\nc\n", "other", Level::Full))
            .unwrap();
        assert_eq!(out.text, "a\nb\nc\n");
    }
}
