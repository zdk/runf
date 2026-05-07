<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="./docs/lowfat_logo_dark.svg">
    <img src="./docs/lowfat_logo_light.svg" alt="lowfat logo" width="700">
  </picture>
</p>

lowfat is a lightweight CLI tool that reduces AI token costs by filtering unnecessary CLI output before it reaches your agent.

Wrap commands as shell functions and pipe them through composable processors like `grep`, `cut`, `head`, and `token-budget`.

<p align="center">
  <img src="docs/demo.gif" alt="lowfat demo: git diff before and after" width="700">
</p>

### Core focus

- **Lightweight** — Small single binary, small core; but extensible.
- **Local-first** — No telemetry; you own your data.
- **Composable** — UNIX-style pipes, mix built-ins and your own filters; not magic.
- **User-owned** — `lowfat history` shows what you run most; allow you to customize for your usecase.

### Install

```sh
cargo install lowfat
# or
brew install zdk/tools/lowfat
```

Pre-built binaries are also available on [GitHub Releases](https://github.com/zdk/lowfat/releases).

### Setup

Choose one of the following:

#### Option A: Claude Code hook

Add to `.claude/settings.json`:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "lowfat hook"
          }
        ]
      }
    ]
  }
}
```

#### Option B: Shell integration

```sh
echo 'eval "$(lowfat shell-init zsh)"' >> ~/.zshrc   # or ~/.bashrc
```

Activates automatically inside agent environments (`CLAUDECODE=1`, `CODEX_ENV`) — commands run normally otherwise.

#### Option C: Direct usage

```sh
lowfat git status
lowfat docker ps
lowfat ls -la
```

#### Intensity levels

Three levels control how aggressively output is compressed:

```sh
lowfat level              # show current level
lowfat level ultra        # set to ultra (most aggressive)
LOWFAT_LEVEL=lite lowfat git log  # per-command override
```

| Level   | Behavior                             |
| ------- | ------------------------------------ |
| `lite`  | Gentle — keeps most context          |
| `full`  | Default — balanced filtering         |
| `ultra` | Maximum compression — minimal output |

#### Inspecting state

| Command          | Shows                                 |
| ---------------- | ------------------------------------- |
| `config`         | resolved config, validates `.lowfat`  |
| `filters`        | enabled/disabled filters              |
| `pipeline <cmd>` | active pipeline for a command         |
| `gain`           | lifetime token savings report         |
| `history`        | plugin candidates, ranked (see below) |
| `audit`          | recent plugin executions              |
| `status`         | compact status badge                  |

### Config file

Optional. Create a `.lowfat` file in your project root (or any parent directory — lowfat walks up to find it). All built-in filters and plugins are active by default.

```sh
# Set intensity level (default: full)
level=ultra

# Filter any command with a pipeline
pipeline.deploy = grep:^(Deploy|ERROR|FAIL) | head:10
```

All settings:

```sh
level=ultra                # lite, full (default), ultra
disable=npm,cargo          # disable specific filters (default: none)
filters=git,docker         # whitelist mode — only these active (default: all)
pipeline.<cmd> = ...       # per-command pipeline
pipeline.<cmd>.error = ... # when exit code != 0
pipeline.<cmd>.empty = ... # when output is empty
pipeline.<cmd>.large = ... # when output > 10KB
```

`disable` and `filters` are mutually exclusive — use one or the other, not both.

Run `lowfat config` to see the resolved config and validate your `.lowfat` file.

All settings can also be overridden with environment variables:

| Env var          | Effect                                                           |
| ---------------- | ---------------------------------------------------------------- |
| `LOWFAT_LEVEL`   | Override level (`lite`, `full`, `ultra`)                         |
| `LOWFAT_DISABLE` | Comma-separated filters to disable                               |
| `LOWFAT_HOME`    | Plugin/config home (default: `~/.lowfat`)                        |
| `LOWFAT_DATA`    | Data directory for history db (default: `~/.local/share/lowfat`) |

Env vars take priority over `.lowfat` file. History and gain data live at `$LOWFAT_DATA/history.db` (default `~/.local/share/lowfat/history.db`) — delete the file to reset.

### Token savings

| Command        | Raw    | Filtered | Saved   |
| -------------- | ------ | -------- | ------- |
| `git status`   | 115t   | 5t       | **96%** |
| `git diff`     | 2,376t | 115t     | **95%** |
| `git log`      | 379t   | 118t     | **69%** |
| `cargo build`  | 558t   | 18t      | **96%** |
| `cargo clippy` | 2,023t | 292t     | **85%** |
| `cargo test`   | 1,499t | 171t     | **88%** |
| `docker ps`    | 271t   | 41t      | **85%** |
| `ls -la`       | 192t   | 30t      | **84%** |

### Find plugin gaps

`lowfat history` ranks your real usage by `runs × avg tokens × (1 − savings)` so commands that run often, produce a lot of output, and aren't being trimmed yet float to the top — exactly the ones worth writing a plugin for.

```
  #  command                    runs    avg raw   savings  plugin
  1  git status                  12x         59     91.5%     yes
  2  ls                           8x        211      0.9%     yes
  3  kubectl get                  6x      4.2K      0.0%      no
  4  terraform plan               3x       12K      0.0%      no
```

`no`-plugin rows are the best candidates. Only `command` + first non-flag arg is stored locally (capped at 10k rows) — never full arguments, output, or secrets.

Prune selectively when the table gets noisy (lifetime `gain` totals are kept intact):

```sh
lowfat history prune                    # default: --older-than 90d
lowfat history prune --older-than 30d   # 30d, 2w, 3m accepted
lowfat history prune --below 2          # drop one-off commands
lowfat history prune --kept-by-plugin   # drop groups already handled by a plugin
lowfat history prune --all              # wipe all invocation rows
lowfat history prune --dry-run [...]    # preview without deleting
```

### Filtering any command

Add a one-liner to `.lowfat` — no plugin needed:

```
# Your deploy script dumps a wall of rollout text
pipeline.deploy = grep:^(Deploy|ERROR|FAIL|Migrating) | head:10

# Custom test runner with non-standard output
pipeline.run-tests = grep:✗|failed|error|^\[suite\] | head:20

# Internal CLI with wide tables — only show what's broken
pipeline.acme = grep:degraded|down|error|total | head:10

# Log viewer spitting thousands of lines
pipeline.stern = grep:ERROR|WARN|panic|fatal | head:30

# CI script that prints every step
pipeline.ci-run = grep:^(STEP|PASS|FAIL|ERROR) | head:20

# Linter with lots of "ok" files
pipeline.lint = grep-v:^✓ | head:30

# Database migration tool
pipeline.migrate = grep:^(Migrating|Applied|Error|Already) | head:15
```

The command name matches what you pass to `lowfat`: `lowfat deploy args...`, `lowfat run-tests --suite integration`, etc. Command names must not contain dots (`.` separates command from condition suffix).

#### Conditional pipelines

Use `.error`, `.empty`, `.large` suffixes to handle different output states:

```
pipeline.deploy = grep:complete|updated | head:5
pipeline.deploy.error = head:50                          # exit code != 0
pipeline.deploy.empty = passthrough                      # no output
pipeline.deploy.large = grep:ERROR|FAIL | token-budget:500  # output > 10KB
```

#### Built-in processors

| Processor        | Syntax                 | Description                                           |
| ---------------- | ---------------------- | ----------------------------------------------------- |
| `grep`           | `grep:pattern`         | Keep lines matching regex                             |
| `grep-v`         | `grep-v:pattern`       | Remove lines matching regex                           |
| `head`           | `head:N`               | First N lines                                         |
| `truncate`       | `truncate:N`           | First N characters per line                           |
| `cut`            | `cut:1,3` or `cut:2-5` | Extract fields (`cut:,;1,3` for comma delimiter)      |
| `strip-ansi`     | `strip-ansi`           | Remove ANSI escape codes                              |
| `token-budget`   | `token-budget:N`       | Trim to ~N tokens                                     |
| `dedup-blank`    | `dedup-blank`          | Collapse consecutive blank lines                      |
| `normalize`      | `normalize`            | Trim whitespace, collapse blanks (runs automatically) |
| `redact-secrets` | `redact-secrets`       | Mask API keys, tokens, passwords                      |

### Plugins

For command-specific filtering beyond built-in processors, plugins are shell scripts that read raw output from stdin and write filtered output to stdout.

Bundled plugins: `git-compact`, `docker-compact`, `ls-compact`, `npm-compact`, `go-compact`, `cargo-compact`

```sh
lowfat plugin list              # list installed plugins
lowfat plugin new terraform     # scaffold a new plugin
lowfat plugin bench terraform   # benchmark against sample files
lowfat plugin doctor            # check plugin health
```

`lowfat plugin new terraform` creates `~/.lowfat/plugins/terraform/terraform-compact/` with:

```
lowfat.toml     # manifest: name, commands, runtime
filter.sh       # your filter logic (stdin → stdout)
samples/        # sample outputs for benchmarking
```

Plugins receive context via environment variables: `$LOWFAT_LEVEL`, `$LOWFAT_COMMAND`, `$LOWFAT_SUBCOMMAND`, `$LOWFAT_ARGS`, `$LOWFAT_EXIT_CODE`.

`$LOWFAT_ARGS` contains all arguments (e.g., `get pods -n kube-system`) — use it when the subcommand alone isn't enough to decide how to filter. See [docs/PLUGINS.md](docs/PLUGINS.md) for examples.

Plugins can be mixed with built-in processors in pipelines:

```
pipeline.git = strip-ansi | git-compact | truncate:100
```

#### Building a plugin with an AI agent

Copy-paste this prompt into Claude Code (or another tool-using agent) and replace `<COMMAND>`:

```
Create a lowfat plugin to filter `<COMMAND>` output for LLM contexts.

Before writing code:
1. Read docs/PLUGINS.md to learn the manifest schema, env-var contract,
   and per-level expectations.
2. Ask me: which subcommands to specialize, and what's noise vs. signal
   in this command's output.

Scaffold at `~/.lowfat/plugins/<COMMAND>/<COMMAND>-compact/`:
- `lowfat.toml` — manifest with `commands` (any aliases too) and the
  agreed `subcommands` list
- `filter.sh` — POSIX sh, `chmod +x`, reads stdin → writes stdout

Filter contract:
- `LOWFAT_LEVEL=ultra` → verdict line(s) only
- `LOWFAT_LEVEL=full`  → strip noise (progress chatter, banner prose),
                         keep diffs / errors / structure
- `LOWFAT_LEVEL=lite`  → gentle trim, higher row caps
- Non-zero `$LOWFAT_EXIT_CODE` → be conservative; preserve error blocks
- Use `$LOWFAT_SUBCOMMAND`, fall back to walking `$LOWFAT_ARGS` when you
  need flags or resource type

Verify:
- `lowfat plugin doctor`            (registers cleanly)
- Drop real captures in `samples/<command>-<sub>-full.txt`
- `lowfat plugin bench <name>`      (aim ≥80% at full on noisy commands)
- Smoke-test: `lowfat <COMMAND> ...` against a real run
```

The agent will ask you what counts as noise — answer with sample output if you have it, then iterate on the bench numbers.

## Alternatives

- [rtk](https://github.com/rtk-ai/rtk)
- [context-mode](https://github.com/mksglu/context-mode)
- [lean-ctx](https://github.com/yvgude/lean-ctx)
- [tokf](https://github.com/mpecan/tokf)
- [tamp](https://github.com/sliday/tamp)
- [ecotokens](https://github.com/hansipie/ecotokens)
- [token-enhancer](https://github.com/xelektron/token-enhancer)

## License

Apache-2.0

## AI notice

Multiple AI tools were used for this project
