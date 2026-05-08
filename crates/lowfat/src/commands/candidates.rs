use anyhow::Result;
use lowfat_core::config::RunfConfig;
use lowfat_core::db::{Db, HistoryRow};

const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";
const CYAN: &str = "\x1b[36m";
const YELLOW: &str = "\x1b[33m";
const MAGENTA: &str = "\x1b[35m";
const WHITE: &str = "\x1b[97m";

/// Per-row diagnosis on two independent axes:
/// `source` — who handled the output (plugin vs lowfat's built-in passthrough);
/// `quality` — how well it filtered (action signal: `weak` rows want tuning).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Status {
    source: Source,
    quality: Quality,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Source {
    /// A registered plugin claims this command+subcommand.
    Plugin,
    /// No plugin handles this row — output is lowfat's built-in passthrough.
    /// This includes the "registered but subcommand not declared" case: the
    /// plugin doesn't actually run for it, so the row is effectively built-in.
    BuiltIn,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Quality {
    /// Savings ≥ 20% — filter (or built-in trim) is pulling its weight.
    Good,
    /// Savings < 20% — needs attention (tune the plugin, or write/extend one).
    Weak,
}

impl Source {
    fn label(self) -> &'static str {
        match self {
            Source::Plugin => "plugin",
            Source::BuiltIn => "built-in",
        }
    }
}

impl Quality {
    fn label(self) -> &'static str {
        match self {
            Quality::Good => "good",
            Quality::Weak => "weak",
        }
    }

    /// Magenta = action item (weak), dim = already handled (good).
    fn color(self) -> &'static str {
        match self {
            Quality::Weak => MAGENTA,
            Quality::Good => DIM,
        }
    }
}

fn classify(r: &HistoryRow) -> Status {
    let source = if r.in_scope_ratio >= 0.5 {
        Source::Plugin
    } else {
        Source::BuiltIn
    };
    let quality = if r.savings_pct >= 20.0 {
        Quality::Good
    } else {
        Quality::Weak
    };
    Status { source, quality }
}

fn fmt_tokens(n: f64) -> String {
    if n >= 1_000_000.0 {
        format!("{:.1}M", n / 1_000_000.0)
    } else if n >= 1_000.0 {
        format!("{:.1}K", n / 1_000.0)
    } else {
        format!("{n:.0}")
    }
}

fn fmt_total(n: u64) -> String {
    fmt_tokens(n as f64)
}

/// Ten-cell bar of post-filter token volume, normalised against the max
/// in the current result set — top row always shows a full bar. Conveys
/// relative pecking order (which rows still consume the most context),
/// not absolute counts (that's what `cost` is for).
fn volume_bar(score: f64, max_score: f64) -> String {
    if max_score <= 0.0 {
        return "░".repeat(10);
    }
    let filled = ((score / max_score) * 10.0).round() as usize;
    let filled = filled.min(10);
    let mut bar = String::with_capacity(10);
    for i in 0..10 {
        bar.push(if i < filled { '█' } else { '░' });
    }
    bar
}

pub fn run(limit: usize, show_all: bool) -> Result<()> {
    let config = RunfConfig::resolve();
    let db = Db::open(&config.data_dir)?;
    let rows = db.history_ranking(limit, show_all)?;

    println!();
    println!("  {BOLD}{WHITE}lowfat{RESET} {DIM}plugin candidates{RESET}");
    println!("  {DIM}─────────────────────────────────────────────────────────{RESET}");
    println!();

    if rows.is_empty() {
        if show_all {
            println!("  {DIM}No data yet. Run some commands through lowfat!{RESET}");
        } else {
            println!(
                "  {DIM}No actionable rows. Re-run with {BOLD}--all{RESET}{DIM} to see every command.{RESET}"
            );
        }
        println!();
        return Ok(());
    }

    let max_score = rows.iter().map(|r| r.score).fold(0.0_f64, f64::max);

    // Header: cost is total raw tokens consumed; volume is the post-filter
    // token volume normalised against max_score (see volume_bar).
    println!(
        "  {DIM}{:>3}  {:<25} {:>5}  {:>8}  {:>8}  {:>8}  {:<8}  {:<6}  {:<6}{RESET}",
        "#", "command", "runs", "avg raw", "cost", "savings", "source", "status", "volume"
    );

    for (i, r) in rows.iter().enumerate() {
        let rank = i + 1;
        let label = if r.subcommand.is_empty() {
            r.command.clone()
        } else {
            format!("{} {}", r.command, r.subcommand)
        };
        let status = classify(r);
        let source_cell = format!("{DIM}{:<8}{RESET}", status.source.label());
        let status_cell = format!(
            "{}{:<6}{RESET}",
            status.quality.color(),
            status.quality.label()
        );
        // Savings pct colour: dim when done, yellow when mid, magenta for weak.
        let save_color = if r.savings_pct >= 50.0 {
            DIM
        } else if r.savings_pct >= 20.0 {
            YELLOW
        } else {
            MAGENTA
        };
        let bar = volume_bar(r.score, max_score);
        println!(
            "  {BOLD}{:>3}{RESET}  {CYAN}{:<25}{RESET} {:>4}x  {:>8}  {:>8}  {save_color}{:>7.1}%{RESET}  {}  {}  {}",
            rank,
            label,
            r.runs,
            fmt_tokens(r.avg_raw_tokens),
            fmt_total(r.total_raw_tokens),
            r.savings_pct,
            source_cell,
            status_cell,
            bar,
        );
    }

    // Footer totals: sum across the shown rows only, so "total" matches the
    // table. Hidden rows (trivia) don't contribute — that's intentional.
    let total_raw: u64 = rows.iter().map(|r| r.total_raw_tokens).sum();
    let total_saved: u64 = rows
        .iter()
        .map(|r| (r.total_raw_tokens as f64 * r.savings_pct / 100.0).round() as u64)
        .sum();
    let total_pct = if total_raw > 0 {
        100.0 * total_saved as f64 / total_raw as f64
    } else {
        0.0
    };

    println!();
    println!(
        "  {DIM}total: {} raw → {} saved ({:.1}%){RESET}",
        fmt_total(total_raw),
        fmt_total(total_saved),
        total_pct
    );
    if !show_all {
        println!(
            "  {DIM}(rows with avg raw <50 tok or <2 runs hidden — pass {BOLD}--all{RESET}{DIM} to see them){RESET}"
        );
    }
    println!();
    println!(
        "  {DIM}Action key:{RESET} {MAGENTA}weak + plugin{RESET}{DIM}   → tune the filter (likely under-matching patterns){RESET}"
    );
    println!(
        "              {MAGENTA}weak + built-in{RESET}{DIM} → scaffold or extend a plugin ({BOLD}lowfat plugin new <cmd>{RESET}{DIM}){RESET}"
    );
    println!();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(cmd: &str, runs: u64, avg_raw: f64, savings: f64, scope: f64) -> HistoryRow {
        HistoryRow {
            command: cmd.into(),
            subcommand: String::new(),
            runs,
            avg_raw_tokens: avg_raw,
            total_raw_tokens: (avg_raw * runs as f64) as u64,
            savings_pct: savings,
            // registered_ratio is no longer read by classify; populate to mirror SQL.
            registered_ratio: scope,
            in_scope_ratio: scope,
            reduced_ratio: if savings > 0.0 { 1.0 } else { 0.0 },
            score: avg_raw * runs as f64 * (1.0 - savings / 100.0),
        }
    }

    #[test]
    fn classify_built_in_when_subcommand_not_in_scope() {
        // terraform (bare): plugin registered but no subcommand match → built-in.
        let s = classify(&row("terraform", 7, 544.0, 33.2, 0.0));
        assert_eq!(s.source, Source::BuiltIn);
        assert_eq!(s.quality, Quality::Good);
    }

    #[test]
    fn classify_weak_when_in_scope_but_low_savings() {
        // git show: in scope, but filter barely trims.
        let s = classify(&row("git", 15, 493.0, 8.8, 1.0));
        assert_eq!(s.source, Source::Plugin);
        assert_eq!(s.quality, Quality::Weak);
    }

    #[test]
    fn classify_good_when_in_scope_and_saving() {
        let s = classify(&row("ls", 109, 109.4, 55.1, 1.0));
        assert_eq!(s.source, Source::Plugin);
        assert_eq!(s.quality, Quality::Good);
    }

    #[test]
    fn classify_built_in_weak_when_no_plugin_and_no_savings() {
        let s = classify(&row("npm", 5, 200.0, 0.0, 0.0));
        assert_eq!(s.source, Source::BuiltIn);
        assert_eq!(s.quality, Quality::Weak);
    }

    #[test]
    fn volume_bar_scales_to_max() {
        assert_eq!(volume_bar(100.0, 100.0), "██████████");
        assert_eq!(volume_bar(50.0, 100.0), "█████░░░░░");
        assert_eq!(volume_bar(0.0, 100.0), "░░░░░░░░░░");
        // Guard: no rows at all → all-empty bar, not a panic.
        assert_eq!(volume_bar(5.0, 0.0), "░░░░░░░░░░");
    }
}
