mod commands;
mod filters;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "lowfat", version)]
#[command(about = "Token-aware command filter for LLM environments")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Command to filter (e.g., lowfat git status)
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Show resolved config and validate .lowfat file
    Config,
    /// List enabled/disabled filters
    Filters {
        /// Print only command names (one per line), for shell-init
        #[arg(long)]
        commands: bool,
    },
    /// Show token savings report
    Gain,
    /// Get or set intensity level
    Level {
        /// Level to set (lite, full, ultra)
        value: Option<String>,
    },
    /// Show status badge
    Status,
    /// Show active pipeline for a command
    Pipeline {
        /// Command to show pipeline for (e.g., git)
        cmd: String,
    },
    /// Local usage history (powers plugin candidate ranking)
    History {
        #[command(subcommand)]
        action: Option<HistoryAction>,
    },
    /// Show plugin audit log
    Audit {
        /// Number of entries to show
        #[arg(default_value = "20")]
        limit: usize,
    },
    /// Claude Code PreToolUse hook (reads JSON from stdin)
    Hook,
    /// Print shell init script for eval
    ShellInit {
        /// Shell type (bash, zsh, fish)
        #[arg(default_value = "zsh")]
        shell: String,
    },
    /// Manage plugins
    Plugin {
        #[command(subcommand)]
        action: PluginAction,
    },
}

#[derive(Subcommand)]
enum HistoryAction {
    /// Rank command usage as plugin candidates
    Candidates {
        /// Number of rows to show
        #[arg(default_value = "20")]
        limit: usize,
    },
    /// Export all invocation rows as JSON to stdout (for backup / analysis)
    Export,
    /// Selectively delete invocation rows (does not touch lifetime gain totals)
    #[command(after_help = "\
Examples:
  lowfat history prune                     # default: --older-than 90d
  lowfat history prune --older-than 30d    # 30d, 2w, 3m suffixes accepted
  lowfat history prune --below 2           # drop groups with fewer than 2 runs
  lowfat history prune --kept-by-plugin    # drop groups already covered by a plugin
  lowfat history prune --all               # wipe all invocation rows
  lowfat history prune --dry-run [...]     # preview without deleting")]
    Prune {
        /// Drop rows older than this duration (e.g. 30d, 2w, 3m). Default if no
        /// other criterion is given: 90d.
        #[arg(long, value_name = "DURATION")]
        older_than: Option<String>,
        /// Drop (command, subcommand) groups with fewer than N runs
        #[arg(long, value_name = "N")]
        below: Option<u64>,
        /// Drop groups where every run was already handled by a plugin
        #[arg(long)]
        kept_by_plugin: bool,
        /// Wipe all invocation rows
        #[arg(long)]
        all: bool,
        /// Report what would be removed without deleting
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum PluginAction {
    /// List community plugins
    List,
    /// Check plugin dependencies
    Doctor,
    /// Show plugin info
    Info { name: String },
    /// Trust a plugin (allow execution)
    Trust { name: String },
    /// Revoke trust for a plugin
    Untrust { name: String },
    /// Benchmark a plugin against its samples
    Bench { name: String },
    /// Scaffold a new plugin
    #[command(after_help = "\
Examples:
  lowfat plugin new cargo                  # creates cargo-compact plugin
  lowfat plugin new kubectl                # creates kubectl-compact plugin
  lowfat plugin new eslint -n eslint-filter  # custom plugin name")]
    New {
        /// Command to intercept (e.g., cargo)
        command: String,
        /// Plugin name override (default: <command>-compact)
        #[arg(short, long)]
        name: Option<String>,
    },
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Some(Commands::Config) => commands::config::run(),
        Some(Commands::Filters { commands }) => commands::filters::run(commands),
        Some(Commands::Hook) => commands::hook::run(),
        Some(Commands::Gain) => commands::gain::run(),
        Some(Commands::Level { value }) => commands::level::run(value.as_deref()),
        Some(Commands::Status) => commands::status::run(),
        Some(Commands::Pipeline { cmd }) => commands::pipeline::run(&cmd),
        Some(Commands::Audit { limit }) => commands::audit::run(limit),
        Some(Commands::History { action }) => match action {
            Some(HistoryAction::Candidates { limit }) => commands::candidates::run(limit),
            Some(HistoryAction::Export) => commands::history_export::run(),
            Some(HistoryAction::Prune {
                older_than,
                below,
                kept_by_plugin,
                all,
                dry_run,
            }) => commands::history_prune::run(commands::history_prune::PruneOpts {
                older_than,
                below,
                kept_by_plugin,
                all,
                dry_run,
            }),
            None => commands::candidates::run(20),
        },
        Some(Commands::ShellInit { shell }) => commands::shell_init::run(&shell),
        Some(Commands::Plugin { action }) => match action {
            PluginAction::List => commands::plugin::list(),
            PluginAction::Doctor => commands::plugin::doctor(),
            PluginAction::Info { name } => commands::plugin::info(&name),
            PluginAction::Trust { name } => commands::plugin::trust(&name),
            PluginAction::Untrust { name } => commands::plugin::untrust(&name),
            PluginAction::Bench { name } => commands::plugin::bench(&name),
            PluginAction::New { command, name } => {
                let plugin_name = name.unwrap_or_else(|| format!("{command}-compact"));
                commands::plugin::new_plugin(&plugin_name, &command)
            }
        },
        None => {
            if cli.args.is_empty() {
                commands::help::run();
                Ok(())
            } else {
                let exit_code = commands::run::run(&cli.args);
                std::process::exit(exit_code);
            }
        }
    };

    if let Err(e) = result {
        eprintln!("lowfat: {e}");
        std::process::exit(1);
    }
}
