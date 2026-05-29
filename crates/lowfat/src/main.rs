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
    // ── primary inspection commands ───────────────────────────────
    /// Show current configuration, active filters, and pipelines
    #[command(after_help = "\
Examples:
  lowfat info              # status badge + active filter list
  lowfat info git          # pipeline applied to `git`
  lowfat info --config     # full resolved config (paths, level, env)")]
    Info {
        /// Show pipeline for this command (e.g., git, docker)
        cmd: Option<String>,
        /// Show full resolved config instead of the default view
        #[arg(long)]
        config: bool,
    },
    /// Show token savings, or recent plugin executions with --audit
    Stats {
        /// Show recent plugin executions instead of savings summary
        #[arg(long)]
        audit: bool,
        /// Number of audit entries (only with --audit)
        #[arg(long, default_value = "20")]
        audit_limit: usize,
    },
    /// Local usage history (powers plugin candidate ranking)
    History {
        /// Number of rows to show (bare form only — equivalent to `candidates --limit`)
        #[arg(long, default_value = "20")]
        limit: usize,
        /// Include trivia rows (bare form only — equivalent to `candidates --all`)
        #[arg(long)]
        all: bool,
        #[command(subcommand)]
        action: Option<HistoryAction>,
    },

    // ── runtime / config ──────────────────────────────────────────
    /// Get or set intensity level
    Level {
        /// Level to set (lite, full, ultra)
        value: Option<String>,
    },

    // ── integrations ──────────────────────────────────────────────
    /// Claude Code PreToolUse hook (reads JSON from stdin)
    Hook,
    /// Rewrite a command to its lowfat-wrapped form (used by agent plugins)
    #[command(after_help = "\
Examples:
  lowfat rewrite git status        # → lowfat git status
  lowfat rewrite curl example.com  # → curl example.com (no filter, unchanged)")]
    Rewrite {
        /// Command to rewrite (e.g., git status)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Manage the OpenCode plugin integration
    Opencode {
        #[command(subcommand)]
        action: OpencodeAction,
    },
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
    /// Run a .lf rule file against stdin (standalone testing)
    #[command(after_help = "\
Examples:
  cat sample.txt | lowfat filter cargo.lf --sub=build --level=ultra
  cat sample.txt | lowfat filter --explain git.lf --sub=diff > /tmp/out
  lowfat filter foo.lf --sub=status --args=\"--porcelain\" < input.txt")]
    Filter {
        /// Path to the .lf file
        path: String,
        /// Subcommand context (sets $sub for the rule)
        #[arg(long, default_value = "")]
        sub: String,
        /// Intensity level
        #[arg(long, default_value = "full")]
        level: String,
        /// Whitespace-separated args (sets $args)
        #[arg(long, default_value = "")]
        args: String,
        /// Exit code of the original command (sets $exit)
        #[arg(long, default_value_t = 0)]
        exit: i32,
        /// Print per-stage diagnostics to stderr
        #[arg(long)]
        explain: bool,
    },

    // ── hidden backward-compat aliases ────────────────────────────
    // Old inspection commands keep working but are hidden from help.
    // Slated for removal one release after .lf migration.
    #[command(hide = true)]
    Config,
    #[command(hide = true)]
    Filters {
        /// Print only command names (one per line), for shell-init
        #[arg(long)]
        commands: bool,
    },
    #[command(hide = true)]
    Gain,
    #[command(hide = true)]
    Status,
    #[command(hide = true)]
    Pipeline {
        /// Command to show pipeline for (e.g., git)
        cmd: String,
    },
    #[command(hide = true)]
    Audit {
        #[arg(default_value = "20")]
        limit: usize,
    },
}

#[derive(Subcommand)]
enum HistoryAction {
    /// Rank command usage as plugin candidates
    Candidates {
        /// Number of rows to show
        #[arg(default_value = "20")]
        limit: usize,
        /// Include trivia rows (avg raw <50 tok or <2 runs)
        #[arg(long)]
        all: bool,
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

#[derive(Subcommand)]
enum OpencodeAction {
    /// Install the plugin to ~/.config/opencode/plugins/lowfat.ts
    Install,
    /// Remove the installed OpenCode plugin
    Uninstall,
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        // ── new consolidated inspection commands ─────────────────
        Some(Commands::Info { cmd, config }) => {
            commands::info::run(cmd.as_deref(), config)
        }
        Some(Commands::Stats { audit, audit_limit }) => {
            commands::stats::run(audit, audit_limit)
        }

        // ── kept ─────────────────────────────────────────────────
        Some(Commands::History { limit, all, action }) => match action {
            Some(HistoryAction::Candidates { limit, all }) => commands::candidates::run(limit, all),
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
            None => commands::candidates::run(limit, all),
        },
        Some(Commands::Level { value }) => commands::level::run(value.as_deref()),
        Some(Commands::Hook) => commands::hook::run(),
        Some(Commands::Rewrite { command }) => commands::rewrite::run(&command),
        Some(Commands::Opencode { action }) => match action {
            OpencodeAction::Install => commands::opencode::install(),
            OpencodeAction::Uninstall => commands::opencode::uninstall(),
        },
        Some(Commands::ShellInit { shell }) => commands::shell_init::run(&shell),
        Some(Commands::Filter {
            path,
            sub,
            level,
            args,
            exit,
            explain,
        }) => commands::filter::run(&path, &sub, &level, &args, exit, explain),
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

        // ── hidden backward-compat aliases route to new code ─────
        Some(Commands::Config) => commands::info::run(None, true),
        Some(Commands::Status) => commands::info::run(None, false),
        Some(Commands::Pipeline { cmd }) => commands::info::run(Some(&cmd), false),
        Some(Commands::Filters { commands: cmds_only }) => {
            // `--commands` is consumed by shell-init scripts; preserve its
            // raw one-per-line output. Bare form is just a view of `info`.
            if cmds_only {
                commands::filters::run(true)
            } else {
                commands::info::run(None, false)
            }
        }
        Some(Commands::Gain) => commands::stats::run(false, 20),
        Some(Commands::Audit { limit }) => commands::stats::run(true, limit),

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
