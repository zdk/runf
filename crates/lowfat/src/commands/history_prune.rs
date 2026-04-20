use anyhow::{bail, Result};
use lowfat_core::config::RunfConfig;
use lowfat_core::db::{Db, PruneFilter};

/// Default `--older-than` when no criterion flag is passed. Conservative enough
/// that running `lowfat history prune` with no args rarely destroys useful data.
const DEFAULT_OLDER_THAN_DAYS: u32 = 90;

pub struct PruneOpts {
    pub older_than: Option<String>,
    pub below: Option<u64>,
    pub kept_by_plugin: bool,
    pub all: bool,
    pub dry_run: bool,
}

pub fn run(opts: PruneOpts) -> Result<()> {
    let filter = resolve_filter(&opts)?;
    let config = RunfConfig::resolve();
    let db = Db::open(&config.data_dir)?;
    let affected = db.prune_invocations(&filter, opts.dry_run)?;

    let verb = if opts.dry_run { "would remove" } else { "removed" };
    println!(
        "lowfat: {verb} {affected} invocation row{s} ({desc})",
        s = if affected == 1 { "" } else { "s" },
        desc = describe_filter(&filter),
    );
    Ok(())
}

fn resolve_filter(opts: &PruneOpts) -> Result<PruneFilter> {
    let set = [
        opts.older_than.is_some(),
        opts.below.is_some(),
        opts.kept_by_plugin,
        opts.all,
    ]
    .iter()
    .filter(|b| **b)
    .count();
    if set > 1 {
        bail!("--older-than, --below, --kept-by-plugin, --all are mutually exclusive");
    }

    if opts.all {
        return Ok(PruneFilter::All);
    }
    if opts.kept_by_plugin {
        return Ok(PruneFilter::KeptByPlugin);
    }
    if let Some(n) = opts.below {
        if n == 0 {
            bail!("--below must be at least 1");
        }
        return Ok(PruneFilter::BelowUsage(n));
    }
    // No flag → default; explicit --older-than → parse it.
    let days = match &opts.older_than {
        Some(s) => parse_duration_days(s)?,
        None => DEFAULT_OLDER_THAN_DAYS,
    };
    Ok(PruneFilter::OlderThan(days))
}

/// Parse "30d", "2w", "3m" → days. Strict: suffix is required so "30" alone
/// doesn't silently get interpreted as anything.
fn parse_duration_days(s: &str) -> Result<u32> {
    let s = s.trim();
    if s.len() < 2 {
        bail!("duration must be like 30d, 2w, or 3m (got {s:?})");
    }
    let (num_part, suffix) = s.split_at(s.len() - 1);
    let mult: u32 = match suffix {
        "d" => 1,
        "w" => 7,
        "m" => 30,
        _ => bail!("duration must end in d, w, or m (got {s:?})"),
    };
    let n: u32 = num_part
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid duration {s:?} (numeric part must be a u32)"))?;
    Ok(n.saturating_mul(mult))
}

fn describe_filter(f: &PruneFilter) -> String {
    match f {
        PruneFilter::All => "all rows".into(),
        PruneFilter::OlderThan(days) => format!("older than {days} days"),
        PruneFilter::BelowUsage(min) => format!("groups with fewer than {min} runs"),
        PruneFilter::KeptByPlugin => "groups fully covered by a plugin".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_parses_common_suffixes() {
        assert_eq!(parse_duration_days("30d").unwrap(), 30);
        assert_eq!(parse_duration_days("2w").unwrap(), 14);
        assert_eq!(parse_duration_days("3m").unwrap(), 90);
    }

    #[test]
    fn duration_rejects_missing_or_bad_suffix() {
        assert!(parse_duration_days("30").is_err());
        assert!(parse_duration_days("30y").is_err());
        assert!(parse_duration_days("").is_err());
        assert!(parse_duration_days("d").is_err());
    }

    #[test]
    fn default_filter_is_older_than_90d() {
        let opts = PruneOpts {
            older_than: None,
            below: None,
            kept_by_plugin: false,
            all: false,
            dry_run: false,
        };
        let filter = resolve_filter(&opts).unwrap();
        assert!(matches!(filter, PruneFilter::OlderThan(90)));
    }

    #[test]
    fn multiple_criteria_rejected() {
        let opts = PruneOpts {
            older_than: Some("30d".into()),
            below: Some(2),
            kept_by_plugin: false,
            all: false,
            dry_run: false,
        };
        assert!(resolve_filter(&opts).is_err());
    }

    #[test]
    fn below_zero_rejected() {
        let opts = PruneOpts {
            older_than: None,
            below: Some(0),
            kept_by_plugin: false,
            all: false,
            dry_run: false,
        };
        assert!(resolve_filter(&opts).is_err());
    }
}
