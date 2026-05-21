//! Redaction ruleset for the `redact-secrets` pipeline processor.
//!
//! Patterns come from three layers, applied in order:
//!   1. built-in defaults — a safe baseline of common secret formats
//!   2. the global rules file  (`~/.lowfat/redact.conf`)
//!   3. the project rules file (`redact.conf` beside `.lowfat`)
//!
//! Why config, not a plugin: redaction is cross-cutting (any command can
//! leak), and an lf-filter plugin degrades to passthrough on error — for
//! redaction that means leaking. The engine here is trusted in-process
//! Rust; only the *patterns* are data, because compliance needs differ
//! per organisation (PCI, HIPAA, internal token formats).

use anyhow::{Context, Result, anyhow, bail};
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, OnceLock};

/// One redaction rule: a compiled regex and its replacement template.
/// The replacement may reference capture groups (`$1`, `${1}`) for
/// partial masking, e.g. keep a prefix and mask the rest.
#[derive(Debug)]
pub struct RedactRule {
    pub re: Regex,
    pub replacement: String,
}

/// The merged, ordered set of redaction rules.
#[derive(Debug)]
pub struct RedactRules {
    rules: Vec<RedactRule>,
}

/// Process-wide ruleset, installed once by [`init`].
static RULES: OnceLock<RedactRules> = OnceLock::new();

/// Defaults-only ruleset — used before [`init`] runs (e.g. in tests).
static DEFAULT_RULES: LazyLock<RedactRules> = LazyLock::new(|| RedactRules {
    rules: RedactRules::compile_defaults(),
});

/// Built-in default secret patterns — the baseline that ships with lowfat.
/// `(regex, replacement)`. Sourced from gitleaks and common secret formats.
fn defaults() -> &'static [(&'static str, &'static str)] {
    &[
        (r"(?i)(AKIA[0-9A-Z]{16})", "[REDACTED:aws-key]"),
        (
            r"(?i)(aws_secret_access_key|aws_secret_key)\s*[=:]\s*\S+",
            "$1=[REDACTED:aws-secret]",
        ),
        (
            r"ghp_[A-Za-z0-9]{36,}|gho_[A-Za-z0-9]{36,}|ghs_[A-Za-z0-9]{36,}|ghr_[A-Za-z0-9]{36,}|github_pat_[A-Za-z0-9_]{22,}",
            "[REDACTED:github-token]",
        ),
        (r"glpat-[A-Za-z0-9\-_]{20,}", "[REDACTED:gitlab-token]"),
        (r"xox[bpsar]-[A-Za-z0-9\-]{24,}", "[REDACTED:slack-token]"),
        (
            r#"(?i)(api[_-]?key|api[_-]?secret|api[_-]?token|access[_-]?token|secret[_-]?key|auth[_-]?token|private[_-]?key)\s*[=:]\s*['"]?([A-Za-z0-9/+=\-_.]{16,})['"]?"#,
            "$1=[REDACTED]",
        ),
        (r"(?i)(Bearer\s+)[A-Za-z0-9\-_.~+/]+=*", "${1}[REDACTED:bearer]"),
        (
            r"eyJ[A-Za-z0-9\-_]+\.eyJ[A-Za-z0-9\-_]+\.[A-Za-z0-9\-_.+/=]+",
            "[REDACTED:jwt]",
        ),
        (
            r"(?s)-----BEGIN[A-Z ]*PRIVATE KEY-----.*?-----END[A-Z ]*PRIVATE KEY-----",
            "[REDACTED:private-key]",
        ),
        (r"(://[^:]+:)[^@\s]+(@)", "${1}[REDACTED]${2}"),
        (r"(?i)(HEROKU_API_KEY)\s*[=:]\s*\S+", "$1=[REDACTED:heroku]"),
        (
            r#"(?i)(secret|token|password|passwd|credential)\s*[=:]\s*['"]?([0-9a-f]{32,})['"]?"#,
            "$1=[REDACTED]",
        ),
    ]
}

impl RedactRules {
    fn compile_defaults() -> Vec<RedactRule> {
        defaults()
            .iter()
            .map(|(p, r)| RedactRule {
                re: Regex::new(p).expect("built-in redact pattern must compile"),
                replacement: r.to_string(),
            })
            .collect()
    }

    /// Parse a `redact.conf` file. Each non-comment line is
    /// `<regex> => <replacement>`. A bare `!no-defaults` line drops the
    /// built-in baseline. Returns the rules and the no-defaults flag.
    fn parse_file(path: &Path) -> Result<(Vec<RedactRule>, bool)> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        let mut rules = Vec::new();
        let mut no_defaults = false;
        for (i, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line == "!no-defaults" {
                no_defaults = true;
                continue;
            }
            let Some((pat, repl)) = line.split_once(" => ") else {
                bail!(
                    "{}:{}: expected `<regex> => <replacement>`, got `{}`",
                    path.display(),
                    i + 1,
                    line
                );
            };
            let pat = pat.trim();
            if pat.is_empty() {
                bail!("{}:{}: empty regex", path.display(), i + 1);
            }
            let re = Regex::new(pat).map_err(|e| {
                anyhow!(
                    "{}:{}: invalid regex `{}`: {}",
                    path.display(),
                    i + 1,
                    pat,
                    e
                )
            })?;
            rules.push(RedactRule {
                re,
                replacement: repl.trim().to_string(),
            });
        }
        Ok((rules, no_defaults))
    }

    /// Load the layered ruleset: built-in defaults, then the global file,
    /// then the project file. A missing file is simply skipped.
    pub fn load(global: Option<&Path>, project: Option<&Path>) -> Result<Self> {
        let mut user_rules = Vec::new();
        let mut no_defaults = false;
        for path in [global, project].into_iter().flatten() {
            if path.is_file() {
                let (mut r, nd) = Self::parse_file(path)?;
                no_defaults |= nd;
                user_rules.append(&mut r);
            }
        }
        let mut rules = if no_defaults {
            Vec::new()
        } else {
            Self::compile_defaults()
        };
        rules.append(&mut user_rules);
        Ok(Self { rules })
    }

    /// Apply every rule, in order, to `text`.
    pub fn apply(&self, text: &str) -> String {
        let mut out = text.to_string();
        for rule in &self.rules {
            out = rule
                .re
                .replace_all(&out, rule.replacement.as_str())
                .into_owned();
        }
        out
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

/// Resolve the global + project `redact.conf` paths from the lowfat home
/// directory and the `.lowfat` config path (if any).
pub fn paths(home_dir: &Path, config_path: Option<&Path>) -> (PathBuf, Option<PathBuf>) {
    let global = home_dir.join("redact.conf");
    let project = config_path
        .and_then(|p| p.parent())
        .map(|d| d.join("redact.conf"));
    (global, project)
}

/// Install the process-wide ruleset. Call once at startup. A malformed
/// `redact.conf` is reported loudly to stderr; lowfat then falls back to
/// the built-in defaults so known secrets are still masked.
pub fn init(global: Option<&Path>, project: Option<&Path>) {
    let rules = match RedactRules::load(global, project) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[lowfat] redact.conf error: {e:#}");
            eprintln!("[lowfat] falling back to built-in redaction defaults");
            RedactRules {
                rules: RedactRules::compile_defaults(),
            }
        }
    };
    let _ = RULES.set(rules);
}

/// Redact secrets from `text` using the installed ruleset, or the
/// built-in defaults if [`init`] has not run.
pub fn redact(text: &str) -> String {
    RULES.get().unwrap_or(&DEFAULT_RULES).apply(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn parse_basic_rule() {
        let dir = tempfile::tempdir().unwrap();
        let f = write(dir.path(), "redact.conf", "# a comment\nFOO-[0-9]+ => [X]\n");
        let (rules, nd) = RedactRules::parse_file(&f).unwrap();
        assert_eq!(rules.len(), 1);
        assert!(!nd);
    }

    #[test]
    fn malformed_regex_errors() {
        let dir = tempfile::tempdir().unwrap();
        let f = write(dir.path(), "redact.conf", "FOO( => [X]\n");
        let err = format!("{:#}", RedactRules::parse_file(&f).unwrap_err());
        assert!(err.contains("invalid regex"), "got: {err}");
    }

    #[test]
    fn missing_separator_errors() {
        let dir = tempfile::tempdir().unwrap();
        let f = write(dir.path(), "redact.conf", "no separator here\n");
        assert!(RedactRules::parse_file(&f).is_err());
    }

    #[test]
    fn layering_defaults_plus_custom() {
        let dir = tempfile::tempdir().unwrap();
        let g = write(dir.path(), "g.conf", "EMP-[0-9]{3} => [EMP]\n");
        let rs = RedactRules::load(Some(&g), None).unwrap();
        let out = rs.apply("key AKIA0000000000000000 staff EMP-123");
        assert!(out.contains("[REDACTED:aws-key]"), "default applied: {out}");
        assert!(out.contains("[EMP]"), "custom applied: {out}");
    }

    #[test]
    fn no_defaults_directive() {
        let dir = tempfile::tempdir().unwrap();
        let g = write(dir.path(), "g.conf", "!no-defaults\nEMP-[0-9]{3} => [EMP]\n");
        let rs = RedactRules::load(Some(&g), None).unwrap();
        let out = rs.apply("AKIA0000000000000000 EMP-123");
        assert!(out.contains("AKIA0000000000000000"), "default dropped: {out}");
        assert!(out.contains("[EMP]"));
    }

    #[test]
    fn project_layers_over_global() {
        let dir = tempfile::tempdir().unwrap();
        let g = write(dir.path(), "g.conf", "!no-defaults\nGLOBAL-X => [G]\n");
        let p = write(dir.path(), "p.conf", "PROJ-Y => [P]\n");
        let rs = RedactRules::load(Some(&g), Some(&p)).unwrap();
        let out = rs.apply("GLOBAL-X and PROJ-Y");
        assert_eq!(out, "[G] and [P]");
    }

    #[test]
    fn capture_group_partial_mask() {
        let dir = tempfile::tempdir().unwrap();
        let f = write(
            dir.path(),
            "c.conf",
            "!no-defaults\n(TOK_)[A-Z0-9]+ => ${1}[REDACTED]\n",
        );
        let rs = RedactRules::load(Some(&f), None).unwrap();
        assert_eq!(rs.apply("TOK_ABC123"), "TOK_[REDACTED]");
    }

    #[test]
    fn missing_file_keeps_defaults() {
        let rs = RedactRules::load(Some(Path::new("/no/such/redact.conf")), None).unwrap();
        assert!(!rs.is_empty());
    }
}
