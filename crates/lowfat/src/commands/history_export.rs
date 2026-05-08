use anyhow::Result;
use lowfat_core::config::RunfConfig;
use lowfat_core::db::Db;
use serde_json::json;

/// Dump all invocation rows as a JSON array to stdout. Pipe to a file for a
/// portable backup: `lowfat history export > invocations.json`.
pub fn run() -> Result<()> {
    let config = RunfConfig::resolve();
    let db = Db::open(&config.data_dir)?;
    let rows = db.export_invocations()?;

    let array: Vec<_> = rows
        .iter()
        .map(|r| {
            json!({
                "timestamp": r.timestamp,
                "command": r.command,
                "subcommand": r.subcommand,
                "raw_tokens": r.raw_tokens,
                "filtered_tokens": r.filtered_tokens,
                "had_plugin": r.had_plugin,
                "in_scope": r.in_scope,
                "reduced": r.reduced,
                "is_external_plugin": r.is_external_plugin,
                "exit_code": r.exit_code,
            })
        })
        .collect();

    println!("{}", serde_json::to_string_pretty(&array)?);
    Ok(())
}
