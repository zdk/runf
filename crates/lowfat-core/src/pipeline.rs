use crate::level::Level;
use crate::tokens::estimate_tokens;
use regex::Regex;

/// A resolved pipeline set: normal chain + optional conditional chains.
#[derive(Debug, Clone)]
pub struct Pipeline {
    pub stages: Vec<PipelineStage>,
}

/// Conditional pipelines: different chains based on exit code or output patterns.
#[derive(Debug, Clone, Default)]
pub struct ConditionalPipelines {
    /// Default pipeline (always present).
    pub default: Option<Pipeline>,
    /// Pipeline when command exits non-zero.
    pub on_error: Option<Pipeline>,
    /// Pipeline when output is empty.
    pub on_empty: Option<Pipeline>,
    /// Pipeline when output exceeds token budget.
    pub on_large: Option<Pipeline>,
}

impl ConditionalPipelines {
    /// Select the right pipeline based on command result.
    pub fn select(&self, exit_code: i32, output: &str) -> Option<&Pipeline> {
        if exit_code != 0 {
            if let Some(ref p) = self.on_error {
                return Some(p);
            }
        }
        if output.is_empty() {
            if let Some(ref p) = self.on_empty {
                return Some(p);
            }
        }
        // "large" = > 1000 tokens
        if estimate_tokens(output) > 1000 {
            if let Some(ref p) = self.on_large {
                return Some(p);
            }
        }
        self.default.as_ref()
    }

    /// Whether any pipelines are configured.
    pub fn is_empty(&self) -> bool {
        self.default.is_none()
            && self.on_error.is_none()
            && self.on_empty.is_none()
            && self.on_large.is_none()
    }
}

/// A single stage in the pipeline.
/// Supports optional parameter via `name:param` syntax (e.g., `truncate:100`, `grep:^error`).
#[derive(Debug, Clone)]
pub struct PipelineStage {
    pub name: String,
    pub stage_type: StageType,
    /// Optional numeric parameter (e.g., line limit, token budget).
    pub param: Option<usize>,
    /// Optional string parameter (e.g., regex pattern for grep).
    pub pattern: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageType {
    /// Handled by `apply_builtin()`, runs in-process.
    Builtin,
    /// External plugin filter (discovered from ~/.lowfat/plugins/)
    Plugin,
}

impl Pipeline {
    /// Create a pipeline with just one filter (backwards-compatible default).
    pub fn single(filter_name: &str) -> Self {
        Pipeline {
            stages: vec![PipelineStage {
                name: filter_name.to_string(),
                stage_type: StageType::Plugin,
                param: None,
                pattern: None,
            }],
        }
    }

    /// Build a pipeline from pre-processors, main filter, and post-processors.
    pub fn from_parts(pre: &[String], filter_name: &str, post: &[String]) -> Self {
        let mut stages: Vec<PipelineStage> = pre.iter().map(|s| parse_pipeline_stage(s)).collect();
        stages.push(PipelineStage {
            name: filter_name.to_string(),
            stage_type: StageType::Plugin,
            param: None,
            pattern: None,
        });
        stages.extend(post.iter().map(|s| parse_pipeline_stage(s)));
        Pipeline { stages }
    }

    /// Parse a pipeline from a pipe-separated string.
    /// e.g., "strip-ansi | grep:^error | cut:1,3 | truncate:100"
    pub fn parse(spec: &str) -> Self {
        let stages = spec
            .split('|')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|raw| parse_pipeline_stage(raw))
            .collect();
        Pipeline { stages }
    }

    pub fn len(&self) -> usize {
        self.stages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stages.is_empty()
    }

    /// Format as display string. Shows params when present (e.g., "truncate:100", "grep:^error").
    pub fn display(&self) -> String {
        self.stages
            .iter()
            .map(|s| {
                if let Some(ref pat) = s.pattern {
                    format!("{}:{}", s.name, pat)
                } else if let Some(p) = s.param {
                    format!("{}:{}", s.name, p)
                } else {
                    s.name.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" → ")
    }
}

/// Parse conditional pipelines from .lowfat config lines.
/// Supports:
///   pipeline.git = strip-ansi | git-compact | truncate
///   pipeline.git.error = strip-ansi | head
///   pipeline.git.empty = passthrough
///   pipeline.git.large = strip-ansi | git-compact | token-budget
pub fn parse_conditional_pipeline(
    lines: &[(String, String)],
) -> ConditionalPipelines {
    let mut cp = ConditionalPipelines::default();
    for (key, spec) in lines {
        match key.as_str() {
            "" => cp.default = Some(Pipeline::parse(spec)),
            "error" => cp.on_error = Some(Pipeline::parse(spec)),
            "empty" => cp.on_empty = Some(Pipeline::parse(spec)),
            "large" => cp.on_large = Some(Pipeline::parse(spec)),
            _ => {} // unknown condition, ignore
        }
    }
    cp
}

/// Parse a single stage spec string into a PipelineStage.
fn parse_pipeline_stage(raw: &str) -> PipelineStage {
    let spec = parse_stage_spec(raw);
    PipelineStage {
        stage_type: resolve_stage_type(&spec.name),
        name: spec.name,
        param: spec.param,
        pattern: spec.pattern,
    }
}

struct ParsedStage {
    name: String,
    param: Option<usize>,
    pattern: Option<String>,
}

/// Parse "name:param" into name + numeric or string param.
/// e.g., "truncate:100" → numeric param, "grep:^error" → string pattern
fn parse_stage_spec(spec: &str) -> ParsedStage {
    match spec.split_once(':') {
        Some((name, rest)) => {
            let name = name.trim().to_string();
            let rest = rest.trim();
            // Try numeric first, fall back to string pattern
            if let Ok(n) = rest.parse::<usize>() {
                ParsedStage { name, param: Some(n), pattern: None }
            } else {
                ParsedStage { name, param: None, pattern: Some(rest.to_string()) }
            }
        }
        None => ParsedStage { name: spec.trim().to_string(), param: None, pattern: None },
    }
}

/// Determine if a stage name is a built-in processor or an external plugin.
fn resolve_stage_type(name: &str) -> StageType {
    match name {
        "strip-ansi" | "truncate" | "token-budget" | "dedup-blank" | "normalize" | "head"
        | "passthrough" | "redact-secrets" | "grep" | "grep-v" | "cut" => {
            StageType::Builtin
        }
        _ => StageType::Plugin,
    }
}

// --- Built-in processors ---

/// Strip ANSI escape codes.
pub fn proc_strip_ansi(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(&c) = chars.peek() {
                    chars.next();
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
                continue;
            }
        }
        result.push(ch);
    }
    result
}

/// Truncate text to N lines.
pub fn proc_truncate(text: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= max_lines {
        return text.to_string();
    }
    let mut result: String = lines[..max_lines].join("\n");
    result.push_str(&format!(
        "\n... ({} lines truncated)",
        lines.len() - max_lines
    ));
    result
}

/// Enforce a token budget.
pub fn proc_token_budget(text: &str, max_tokens: usize) -> String {
    let current = estimate_tokens(text);
    if current <= max_tokens {
        return text.to_string();
    }
    let ratio = max_tokens as f64 / current as f64;
    let target_chars = (text.len() as f64 * ratio) as usize;
    let mut result = text[..target_chars.min(text.len())].to_string();
    if let Some(pos) = result.rfind('\n') {
        result.truncate(pos);
    }
    let truncated_tokens = estimate_tokens(&result);
    result.push_str(&format!(
        "\n... (truncated to ~{} tokens from {})",
        truncated_tokens, current
    ));
    result
}

/// Remove consecutive blank lines (keep max 1).
pub fn proc_dedup_blank(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut prev_blank = false;
    for line in text.lines() {
        if line.trim().is_empty() {
            if !prev_blank {
                result.push('\n');
                prev_blank = true;
            }
        } else {
            result.push_str(line);
            result.push('\n');
            prev_blank = false;
        }
    }
    result
}

/// Normalize whitespace: trim trailing spaces per line, collapse consecutive
/// blank lines, and strip leading/trailing blank lines from the whole output.
pub fn proc_normalize(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut prev_blank = false;

    for line in text.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            if !prev_blank && !result.is_empty() {
                result.push('\n');
                prev_blank = true;
            }
        } else {
            result.push_str(trimmed);
            result.push('\n');
            prev_blank = false;
        }
    }

    // Strip trailing blank lines
    while result.ends_with("\n\n") {
        result.pop();
    }
    result
}

/// Redact secrets via the configured ruleset — built-in defaults layered
/// with `redact.conf` (global + project). See [`crate::redact`].
pub fn proc_redact_secrets(text: &str) -> String {
    crate::redact::redact(text)
}

/// Keep or reject lines matching a regex pattern.
/// `invert = false` → grep (keep matches), `invert = true` → grep -v (reject matches).
pub fn proc_grep(text: &str, pattern: &str, invert: bool) -> String {
    let re = match Regex::new(pattern) {
        Ok(r) => r,
        Err(_) => return text.to_string(),
    };
    text.lines()
        .filter(|line| re.is_match(line) != invert)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Extract fields from each line. Unix `cut -f` compatible syntax:
///   1-indexed, supports ranges (N-M), open-ended (N-), and comma-separated lists.
///
/// Spec: `[delimiter;]fields`
///   `1,3`       — whitespace split, fields 1 and 3
///   `1-3`       — whitespace split, fields 1 through 3
///   `2-`        — field 2 to end
///   `:;1,3`     — colon delimiter, fields 1 and 3
///   `/;2-4`     — slash delimiter, fields 2 through 4
pub fn proc_cut(text: &str, spec: &str) -> String {
    let (delim, field_spec) = match spec.split_once(';') {
        Some((d, f)) => (Some(d), f),
        None => (None, spec),
    };

    // Parse field spec into list of (start, end) inclusive ranges.
    // end=usize::MAX means "to end of line".
    let ranges: Vec<(usize, usize)> = field_spec
        .split(',')
        .filter_map(|s| {
            let s = s.trim();
            if let Some((a, b)) = s.split_once('-') {
                let start = a.parse::<usize>().ok()?;
                let end = if b.is_empty() { usize::MAX } else { b.parse::<usize>().ok()? };
                Some((start, end))
            } else {
                let n = s.parse::<usize>().ok()?;
                Some((n, n))
            }
        })
        .collect();
    if ranges.is_empty() {
        return text.to_string();
    }

    text.lines()
        .map(|line| {
            let parts: Vec<&str> = match delim {
                Some(d) => line.split(d).collect(),
                None => line.split_whitespace().collect(),
            };
            let n = parts.len();
            let mut selected = Vec::new();
            for &(start, end) in &ranges {
                let end = end.min(n);
                for i in start..=end {
                    if let Some(&field) = parts.get(i.checked_sub(1).unwrap_or(0)) {
                        if i >= 1 { selected.push(field); }
                    }
                }
            }
            selected.join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Apply a built-in processor by name. Returns None if not a built-in.
/// When `param` is Some, it overrides the level-based default.
/// When `pattern` is Some, it provides a string param (regex for grep, field spec for cut).
pub fn apply_builtin(name: &str, text: &str, level: Level, param: Option<usize>, pattern: Option<&str>) -> Option<String> {
    match name {
        "strip-ansi" => Some(proc_strip_ansi(text)),
        "truncate" => {
            let limit = param.unwrap_or_else(|| level.head_limit(200));
            Some(proc_truncate(text, limit))
        }
        "head" => {
            let limit = param.unwrap_or_else(|| level.head_limit(40));
            Some(proc_truncate(text, limit))
        }
        "token-budget" => {
            let budget = param.unwrap_or_else(|| match level {
                Level::Lite => 2000,
                Level::Full => 1000,
                Level::Ultra => 500,
            });
            Some(proc_token_budget(text, budget))
        }
        "dedup-blank" => Some(proc_dedup_blank(text)),
        "normalize" => Some(proc_normalize(text)),
        "redact-secrets" => Some(proc_redact_secrets(text)),
        "grep" => {
            let pat = pattern.unwrap_or(".");
            Some(proc_grep(text, pat, false))
        }
        "grep-v" => {
            // No pattern → no-op (keep all lines)
            let pat = pattern.unwrap_or("(?!.*)");
            Some(proc_grep(text, pat, true))
        }
        "cut" => {
            let spec = pattern.unwrap_or("1-");
            Some(proc_cut(text, spec))
        }
        "passthrough" => Some(text.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_single() {
        let p = Pipeline::single("git-compact");
        assert_eq!(p.len(), 1);
        assert_eq!(p.stages[0].name, "git-compact");
        assert_eq!(p.display(), "git-compact");
    }

    #[test]
    fn pipeline_from_parts() {
        let p = Pipeline::from_parts(
            &["strip-ansi".to_string()],
            "git-compact",
            &["truncate".to_string()],
        );
        assert_eq!(p.len(), 3);
        assert_eq!(p.stages[0].stage_type, StageType::Builtin);
        assert_eq!(p.stages[1].stage_type, StageType::Plugin);
        assert_eq!(p.stages[2].stage_type, StageType::Builtin);
        assert_eq!(p.display(), "strip-ansi → git-compact → truncate");
    }

    #[test]
    fn pipeline_parse() {
        let p = Pipeline::parse("strip-ansi | git-compact | truncate");
        assert_eq!(p.len(), 3);
        assert_eq!(p.stages[0].name, "strip-ansi");
        assert_eq!(p.stages[1].name, "git-compact");
        assert_eq!(p.stages[2].name, "truncate");
    }

    #[test]
    fn conditional_select_default() {
        let cp = ConditionalPipelines {
            default: Some(Pipeline::single("git-compact")),
            ..Default::default()
        };
        let p = cp.select(0, "some output").unwrap();
        assert_eq!(p.stages[0].name, "git-compact");
    }

    #[test]
    fn conditional_select_error() {
        let cp = ConditionalPipelines {
            default: Some(Pipeline::single("git-compact")),
            on_error: Some(Pipeline::parse("strip-ansi | head")),
            ..Default::default()
        };
        // exit_code != 0 → on_error
        let p = cp.select(1, "error output").unwrap();
        assert_eq!(p.display(), "strip-ansi → head");
        // exit_code == 0 → default
        let p = cp.select(0, "ok output").unwrap();
        assert_eq!(p.display(), "git-compact");
    }

    #[test]
    fn conditional_select_large() {
        let cp = ConditionalPipelines {
            default: Some(Pipeline::single("git-compact")),
            on_large: Some(Pipeline::parse("git-compact | token-budget")),
            ..Default::default()
        };
        let large_output = "x".repeat(5000); // > 1000 tokens
        let p = cp.select(0, &large_output).unwrap();
        assert_eq!(p.display(), "git-compact → token-budget");
    }

    #[test]
    fn conditional_select_empty() {
        let cp = ConditionalPipelines {
            default: Some(Pipeline::single("git-compact")),
            on_empty: Some(Pipeline::parse("passthrough")),
            ..Default::default()
        };
        let p = cp.select(0, "").unwrap();
        assert_eq!(p.display(), "passthrough");
    }

    #[test]
    fn conditional_parse() {
        let lines = vec![
            ("".to_string(), "strip-ansi | git-compact".to_string()),
            ("error".to_string(), "head".to_string()),
            ("large".to_string(), "git-compact | token-budget".to_string()),
        ];
        let cp = parse_conditional_pipeline(&lines);
        assert!(cp.default.is_some());
        assert!(cp.on_error.is_some());
        assert!(cp.on_large.is_some());
        assert!(cp.on_empty.is_none());
    }

    #[test]
    fn strip_ansi_basic() {
        let input = "\x1b[31mERROR\x1b[0m: something failed";
        assert_eq!(proc_strip_ansi(input), "ERROR: something failed");
    }

    #[test]
    fn strip_ansi_clean() {
        assert_eq!(proc_strip_ansi("no escape codes"), "no escape codes");
    }

    #[test]
    fn truncate_within_limit() {
        let input = "line1\nline2\nline3";
        assert_eq!(proc_truncate(input, 5), input);
    }

    #[test]
    fn truncate_over_limit() {
        let input = "line1\nline2\nline3\nline4\nline5";
        let result = proc_truncate(input, 3);
        assert!(result.starts_with("line1\nline2\nline3"));
        assert!(result.contains("2 lines truncated"));
    }

    #[test]
    fn token_budget_within() {
        assert_eq!(proc_token_budget("short", 100), "short");
    }

    #[test]
    fn token_budget_over() {
        let input = "a".repeat(400); // 100 tokens
        let result = proc_token_budget(&input, 50);
        assert!(result.len() < input.len());
        assert!(result.contains("truncated to"));
    }

    #[test]
    fn dedup_blank() {
        let input = "line1\n\n\n\nline2\n\nline3";
        assert_eq!(proc_dedup_blank(input), "line1\n\nline2\n\nline3\n");
    }

    #[test]
    fn normalize_trailing_spaces() {
        let input = "line1   \nline2\t\t\n  line3  ";
        assert_eq!(proc_normalize(input), "line1\nline2\n  line3\n");
    }

    #[test]
    fn normalize_blank_lines() {
        let input = "line1\n\n\n\nline2\n\nline3\n\n\n";
        assert_eq!(proc_normalize(input), "line1\n\nline2\n\nline3\n");
    }

    #[test]
    fn normalize_leading_blanks() {
        let input = "\n\n\nline1\nline2";
        assert_eq!(proc_normalize(input), "line1\nline2\n");
    }

    #[test]
    fn normalize_empty() {
        assert_eq!(proc_normalize(""), "");
        assert_eq!(proc_normalize("\n\n\n"), "");
    }

    #[test]
    fn apply_builtin_known() {
        assert!(apply_builtin("strip-ansi", "t", Level::Full, None, None).is_some());
        assert!(apply_builtin("truncate", "t", Level::Full, None, None).is_some());
        assert!(apply_builtin("token-budget", "t", Level::Full, None, None).is_some());
        assert!(apply_builtin("dedup-blank", "t", Level::Full, None, None).is_some());
        assert!(apply_builtin("normalize", "t", Level::Full, None, None).is_some());
        assert!(apply_builtin("head", "t", Level::Full, None, None).is_some());
        assert!(apply_builtin("passthrough", "t", Level::Full, None, None).is_some());
    }

    #[test]
    fn apply_builtin_unknown() {
        assert!(apply_builtin("git-compact", "t", Level::Full, None, None).is_none());
    }

    #[test]
    fn parse_parameterized_stages() {
        let p = Pipeline::parse("strip-ansi | truncate:100 | token-budget:1500");
        assert_eq!(p.len(), 3);
        assert_eq!(p.stages[0].name, "strip-ansi");
        assert_eq!(p.stages[0].param, None);
        assert_eq!(p.stages[1].name, "truncate");
        assert_eq!(p.stages[1].param, Some(100));
        assert_eq!(p.stages[2].name, "token-budget");
        assert_eq!(p.stages[2].param, Some(1500));
    }

    #[test]
    fn display_with_params() {
        let p = Pipeline::parse("strip-ansi | git-compact | truncate:100");
        assert_eq!(p.display(), "strip-ansi → git-compact → truncate:100");
    }

    #[test]
    fn param_overrides_level_default() {
        let lines = (0..500).map(|i| format!("line{i}")).collect::<Vec<_>>().join("\n");
        // Without param: Full level truncate = 200 lines
        let default_result = apply_builtin("truncate", &lines, Level::Full, None, None).unwrap();
        assert!(default_result.contains("truncated"));
        // With param: override to 50 lines
        let custom_result = apply_builtin("truncate", &lines, Level::Full, Some(50), None).unwrap();
        assert!(custom_result.contains("truncated"));
        assert!(custom_result.lines().count() < default_result.lines().count());
    }

    #[test]
    fn redact_aws_key() {
        let input = "key=AKIAIOSFODNN7EXAMPLE";
        let out = proc_redact_secrets(input);
        assert!(out.contains("[REDACTED:aws-key]"));
        assert!(!out.contains("AKIAIOSFODNN7EXAMPLE"));
    }

    #[test]
    fn redact_github_token() {
        let input = "token: ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij";
        let out = proc_redact_secrets(input);
        assert!(out.contains("[REDACTED:github-token]"));
        assert!(!out.contains("ghp_"));
    }

    #[test]
    fn redact_jwt() {
        let input = "auth: eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abc123signature";
        let out = proc_redact_secrets(input);
        assert!(out.contains("[REDACTED:jwt]"));
        assert!(!out.contains("eyJhbGci"));
    }

    #[test]
    fn redact_bearer_token() {
        let input = "Authorization: Bearer eytoken123456.abcdef.xyz";
        let out = proc_redact_secrets(input);
        assert!(out.contains("[REDACTED:bearer]"));
    }

    #[test]
    fn redact_password_in_url() {
        let input = "postgres://admin:s3cretP4ss@db.example.com:5432/mydb";
        let out = proc_redact_secrets(input);
        assert!(out.contains("[REDACTED]@"));
        assert!(!out.contains("s3cretP4ss"));
    }

    #[test]
    fn redact_private_key() {
        let input = "-----BEGIN RSA PRIVATE KEY-----\nMIIBogIB...\n-----END RSA PRIVATE KEY-----";
        let out = proc_redact_secrets(input);
        assert!(out.contains("[REDACTED:private-key]"));
        assert!(!out.contains("MIIBogIB"));
    }

    #[test]
    fn redact_generic_api_key() {
        let input = "API_KEY=abcdef1234567890abcdef1234567890";
        let out = proc_redact_secrets(input);
        assert!(out.contains("[REDACTED]"));
        assert!(!out.contains("abcdef1234567890abcdef1234567890"));
    }

    #[test]
    fn redact_slack_token() {
        // Build the token at runtime to avoid GitHub push protection flagging it
        let token = format!("xoxb-{}-{}-{}", "0".repeat(12), "0".repeat(13), "a".repeat(24));
        let input = format!("SLACK_TOKEN={token}");
        let out = proc_redact_secrets(&input);
        assert!(out.contains("[REDACTED:slack-token]"));
    }

    #[test]
    fn redact_preserves_normal_text() {
        let input = "commit abc123\nAuthor: zdk\n\n    fix login bug\n";
        let out = proc_redact_secrets(input);
        assert_eq!(out, input);
    }

    #[test]
    fn redact_secrets_is_builtin() {
        assert!(apply_builtin("redact-secrets", "test", Level::Full, None, None).is_some());
    }

    #[test]
    fn grep_keeps_matching_lines() {
        let input = "error: bad\ninfo: ok\nerror: worse\nwarn: meh";
        let result = proc_grep(input, "^error", false);
        assert_eq!(result, "error: bad\nerror: worse");
    }

    #[test]
    fn grep_v_removes_matching_lines() {
        let input = "error: bad\ninfo: ok\nerror: worse\nwarn: meh";
        let result = proc_grep(input, "^error", true);
        assert_eq!(result, "info: ok\nwarn: meh");
    }

    #[test]
    fn grep_invalid_regex_passthrough() {
        let input = "hello\nworld";
        let result = proc_grep(input, "[invalid", false);
        assert_eq!(result, input);
    }

    #[test]
    fn grep_via_apply_builtin() {
        let input = "  M src/main.rs\n?? temp.txt\n  D old.rs";
        let result = apply_builtin("grep", input, Level::Full, None, Some("^\\s*[MADRCU?!]")).unwrap();
        assert_eq!(result, "  M src/main.rs\n?? temp.txt\n  D old.rs");
    }

    #[test]
    fn grep_v_via_apply_builtin() {
        let input = "index abc123..def456\nmode 100644\n+++ b/file.rs\n--- a/file.rs";
        let result = apply_builtin("grep-v", input, Level::Full, None, Some("^(index |mode )")).unwrap();
        assert_eq!(result, "+++ b/file.rs\n--- a/file.rs");
    }

    #[test]
    fn grep_pipeline_parse() {
        let p = Pipeline::parse("grep:^error | head:10");
        assert_eq!(p.stages[0].name, "grep");
        assert_eq!(p.stages[0].pattern.as_deref(), Some("^error"));
        assert_eq!(p.stages[0].stage_type, StageType::Builtin);
        assert_eq!(p.stages[1].name, "head");
        assert_eq!(p.stages[1].param, Some(10));
    }

    #[test]
    fn cut_single_field() {
        let input = "alice 100 x\nbob 200 y\ncharlie 300 z";
        assert_eq!(proc_cut(input, "2"), "100\n200\n300");
    }

    #[test]
    fn cut_multiple_fields() {
        let input = "alice 100 x\nbob 200 y";
        assert_eq!(proc_cut(input, "1,3"), "alice x\nbob y");
    }

    #[test]
    fn cut_range() {
        let input = "a b c d e";
        assert_eq!(proc_cut(input, "2-4"), "b c d");
    }

    #[test]
    fn cut_open_ended_range() {
        let input = "a b c d e";
        assert_eq!(proc_cut(input, "3-"), "c d e");
    }

    #[test]
    fn cut_custom_delimiter() {
        let input = "alice:100:x\nbob:200:y";
        assert_eq!(proc_cut(input, ":;1,3"), "alice x\nbob y");
    }

    #[test]
    fn cut_via_apply_builtin() {
        let input = "alice 100 x\nbob 200 y";
        let result = apply_builtin("cut", input, Level::Full, None, Some("1,2")).unwrap();
        assert_eq!(result, "alice 100\nbob 200");
    }

    #[test]
    fn cut_pipeline_parse() {
        let p = Pipeline::parse("cut:1,3 | head:10");
        assert_eq!(p.stages.len(), 2);
        assert_eq!(p.stages[0].name, "cut");
        assert_eq!(p.stages[0].pattern.as_deref(), Some("1,3"));
        assert_eq!(p.stages[1].name, "head");
        assert_eq!(p.stages[1].param, Some(10));
    }
}
