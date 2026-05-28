//! End-to-end check that the bundled .lf plugins match the .sh
//! baselines they replaced on real captured samples.
//!
//! The .sh and .lf shouldn't produce byte-identical output (different
//! trailing whitespace conventions), but token counts should track
//! within a small percentage. This catches regressions where a .lf
//! rewrite accidentally drops or keeps the wrong category of lines.

use lowfat_core::level::Level;
use lowfat_core::lf::{self, ExecCtx};
use lowfat_core::tokens::estimate_tokens;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn repo_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// Run the .sh filter with env vars set the way the real lowfat runner
/// would set them. Returns (stdout, stderr) — we ignore stderr.
fn run_sh(filter: &Path, sample: &str, command: &str, sub: &str, level: Level) -> String {
    let mut child = Command::new("sh")
        .arg(filter)
        .env("LOWFAT_LEVEL", level.to_string())
        .env("LOWFAT_COMMAND", command)
        .env("LOWFAT_SUBCOMMAND", sub)
        .env("LOWFAT_ARGS", "")
        .env("LOWFAT_EXIT_CODE", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn sh filter");
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(sample.as_bytes()).unwrap();
    }
    let out = child.wait_with_output().expect("wait sh filter");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn run_lf(filter: &Path, sample: &str, sub: &str, level: Level) -> String {
    let src = std::fs::read_to_string(filter).expect("read .lf");
    let rs = lf::parse(&src).expect("parse .lf");
    let args: Vec<String> = vec![];
    let ctx = ExecCtx {
        sub,
        level,
        exit_code: 0,
        args: &args,
    };
    lf::execute(&rs, &ctx, sample).expect("execute .lf")
}

/// Parse a sample filename like `git-status-full.txt` into
/// (command, subcommand, level).
fn parse_sample(stem: &str) -> (String, String, Level) {
    let parts: Vec<&str> = stem.split('-').collect();
    let level = match parts.last().copied() {
        Some("ultra") => Level::Ultra,
        Some("lite") => Level::Lite,
        _ => Level::Full,
    };
    let cmd = parts[0].to_string();
    let sub = if parts.len() >= 3 {
        parts[1..parts.len() - 1].join("-")
    } else if parts.len() == 2 {
        parts[1].to_string()
    } else {
        String::new()
    };
    (cmd, sub, level)
}

/// For each `<plugin>/samples/*.txt`, run both .sh and .lf and check
/// the output token counts match within `tolerance_pct`.
fn check_plugin(plugin_dir: &Path, tolerance_pct: f64) {
    let samples = plugin_dir.join("samples");
    let sh = plugin_dir.join("filter.sh");
    let lf = plugin_dir.join("filter.lf");
    assert!(sh.is_file(), "missing {}", sh.display());
    assert!(lf.is_file(), "missing {}", lf.display());

    let entries: Vec<_> = std::fs::read_dir(&samples)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|x| x == "txt")
                .unwrap_or(false)
        })
        .collect();
    assert!(!entries.is_empty(), "no samples in {}", samples.display());

    for e in entries {
        let path = e.path();
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        let (cmd, sub, level) = parse_sample(&stem);
        let raw = std::fs::read_to_string(&path).unwrap();

        let sh_out = run_sh(&sh, &raw, &cmd, &sub, level);
        let lf_out = run_lf(&lf, &raw, &sub, level);

        let sh_t = estimate_tokens(&sh_out) as f64;
        let lf_t = estimate_tokens(&lf_out) as f64;

        // Allow exact-zero-vs-tiny differences (the "clean" fallback case).
        let denom = sh_t.max(lf_t).max(5.0);
        let diff_pct = ((sh_t - lf_t).abs() / denom) * 100.0;
        assert!(
            diff_pct <= tolerance_pct,
            "{stem}: sh={sh_t}t lf={lf_t}t differ {:.1}% (>{:.1}%)\n--- sh ---\n{sh_out}\n--- lf ---\n{lf_out}",
            diff_pct,
            tolerance_pct
        );
    }
}

// Shipped plugins are embedded in the lowfat-plugin crate. The tool-specific
// fixtures below (cargo/go/npm/kubectl) are bench/parity test data only — not
// shipped; installable versions live in the community repo.
fn bundled_dir() -> PathBuf {
    repo_root().join("crates/lowfat-plugin/embedded")
}

fn fixtures_dir() -> PathBuf {
    repo_root().join("test-fixtures/plugins")
}

#[test]
fn git_compact_parity() {
    check_plugin(&bundled_dir().join("git/git-compact"), 5.0);
}

// `git diff --stat` (and --name-only / --shortstat) emit no `diff `/`@@ `
// markers, so compact-diff produces nothing. Both filters must fall back to
// the blank-stripped raw output instead of returning empty — otherwise the
// diffstat is silently dropped. Driven directly rather than via a sample file
// because the filename parser would read the subcommand as `diff-stat`.
#[test]
fn git_diff_stat_fallback() {
    let plugin = bundled_dir().join("git/git-compact");
    let sh = plugin.join("filter.sh");
    let lf = plugin.join("filter.lf");

    let stat = " README.md                                | 1 +\n\
                 crates/lowfat-runner/tests/lf_plugins.rs | 42 ++++++++++++++++++++++++\n\
                 2 files changed, 43 insertions(+), 0 deletions(-)\n";

    for level in [Level::Ultra, Level::Lite, Level::Full] {
        let sh_out = run_sh(&sh, stat, "git", "diff", level);
        let lf_out = run_lf(&lf, stat, "diff", level);

        assert!(
            sh_out.contains("files changed"),
            "sh dropped diffstat at {level}:\n{sh_out}"
        );
        assert!(
            lf_out.contains("files changed") && lf_out.contains("README.md"),
            "lf dropped diffstat at {level}:\n{lf_out}"
        );
    }
}

#[test]
fn cargo_compact_parity() {
    check_plugin(&fixtures_dir().join("cargo/cargo-compact"), 10.0);
}

#[test]
fn docker_compact_parity() {
    check_plugin(&bundled_dir().join("docker/docker-compact"), 10.0);
}

#[test]
fn ls_compact_parity() {
    check_plugin(&bundled_dir().join("ls/ls-compact"), 5.0);
}

#[test]
fn npm_compact_parity() {
    check_plugin(&fixtures_dir().join("npm/npm-compact"), 10.0);
}

#[test]
fn go_compact_parity() {
    check_plugin(&fixtures_dir().join("go/go-compact"), 10.0);
}
