# Configuring lowfat

How to control what gets filtered, how aggressively, and where to look when something isn't behaving as expected. For writing plugins, see [PLUGINS.md](PLUGINS.md).

## Intensity levels

Three levels control how aggressively output is compressed:

| Level   | Behavior                             |
| ------- | ------------------------------------ |
| `lite`  | Gentle — keeps most context          |
| `full`  | Default — balanced filtering         |
| `ultra` | Maximum compression — minimal output |

```sh
lowfat level              # show current level
lowfat level ultra        # set to ultra (most aggressive)
LOWFAT_LEVEL=lite lowfat git log  # per-command override
```

Plugins read `$LOWFAT_LEVEL` (in `.sh`) or `$level` (in `.lf`) and tune row caps / drop-patterns accordingly.

## Inspecting state

| Command       | Shows                                                                                            |
| ------------- | ------------------------------------------------------------------------------------------------ |
| `info`        | status badge + active filters; with `<cmd>`, the pipeline; with `--config`, full resolved config |
| `stats`       | lifetime token savings; with `--audit`, recent plugin executions                                 |
| `history`     | plugin candidates ranked by cost; plus `prune` / `export` subcommands                            |

## The `.lowfat` config file

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

Run `lowfat info --config` to see the resolved config and validate your `.lowfat` file.

## Environment variables

| Env var             | Effect                                                              |
| ------------------- | ------------------------------------------------------------------- |
| `LOWFAT_LEVEL`      | Override level (`lite`, `full`, `ultra`)                            |
| `LOWFAT_DISABLE`    | Comma-separated filters to disable                                  |
| `LOWFAT_HOME`       | Plugin/config home — overrides the resolution order below           |
| `XDG_CONFIG_HOME`   | If set, plugin/config home is `$XDG_CONFIG_HOME/lowfat`             |
| `LOWFAT_DATA`       | Data directory for history db (default: `~/.local/share/lowfat`)    |

Env vars take priority over `.lowfat`. History and gain data live at `$LOWFAT_DATA/history.db` (default `~/.local/share/lowfat/history.db`) — delete the file to reset.

### Config home and precedence

The plugin/config home is resolved in this order — first match wins:

1. **`$LOWFAT_HOME`** — explicit override
2. **`$XDG_CONFIG_HOME/lowfat`** when `XDG_CONFIG_HOME` is set
3. **`~/.config/lowfat`** if that directory already exists (XDG default)
4. **`~/.lowfat`** — fallback when none of the above apply

To move an existing install to the XDG location: `mkdir -p ~/.config/lowfat && mv ~/.lowfat/* ~/.config/lowfat/ && rmdir ~/.lowfat`. If both paths exist, the XDG path wins and lowfat prints a one-line warning to stderr.

## Filtering any command without writing a plugin

Add a one-liner to `.lowfat`:

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

### Conditional pipelines

Use `.error`, `.empty`, `.large` suffixes to handle different output states:

```
pipeline.deploy = grep:complete|updated | head:5
pipeline.deploy.error = head:50                          # exit code != 0
pipeline.deploy.empty = passthrough                      # no output
pipeline.deploy.large = grep:ERROR|FAIL | token-budget:500  # output > 10KB
```

### Built-in processors

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
| `redact-secrets` | `redact-secrets`       | Mask secrets — built-in patterns + custom `redact.conf` |

Built-ins can be mixed with plugins in pipelines:

```
pipeline.git = strip-ansi | git-compact | truncate:100
```

### Custom redaction — `redact.conf`

`redact-secrets` ships defaults for common secrets (AWS / GitHub / GitLab / Slack keys, JWTs, bearer tokens, PEM private keys, passwords in URLs). Compliance patterns differ per organisation, so add your own in a `redact.conf`:

- **Global** — `~/.lowfat/redact.conf` (org-wide policy)
- **Project** — `redact.conf` beside `.lowfat` (repo-specific, version-controlled)

Both layer on top of the defaults. One rule per line, `<regex> => <replacement>`:

```
# redact.conf
EMP-[0-9]{6}                => [EMPLOYEE-ID]
(MRN:?\s*)[0-9]{8}          => ${1}[REDACTED:mrn]
[A-Za-z0-9._%+-]+@acme\.com => [REDACTED:email]
```

`$1` / `${1}` reference capture groups for partial masking; `#` starts a comment. A bare `!no-defaults` line drops the built-in baseline — use it when compliance requires *only* your patterns.

Redaction runs in lowfat's trusted core, so — unlike a plugin — it can't degrade to passthrough. A malformed pattern is reported on stderr; lowfat then falls back to the built-in defaults until you fix the file.

## Finding plugin gaps with `history`

`lowfat history` ranks your real usage by `cost = runs × avg tokens × (1 − savings)` so commands that run often, produce a lot of output, and aren't being trimmed yet float to the top — exactly the ones worth writing (or tightening) a plugin for.

```
  #  command                    runs   avg raw      cost   savings  source    status  volume
  1  ls                         299x        91     27.3K     51.2%  built-in  good    ██████████
  2  git show                    32x       608     19.5K     39.5%  built-in  good    █████████░
  3  git                         74x       770     57.0K     83.6%  built-in  good    ███████░░░
  4  docker                      68x      4.7K    320.1K     97.4%  built-in  good    ██████░░░░
  5  git diff                    27x       234      6.3K     15.9%  built-in  weak    ████░░░░░░
  …
total: 479.6K raw → 413.4K saved (86.2%)
```

`weak`-status rows are the best targets — high cost relative to current savings. `source` shows whether trimming is coming from a built-in or an external plugin. Only `command` + first non-flag arg is stored locally (capped at 10k rows) — never full arguments, output, or secrets.

Prune selectively when the table gets noisy (lifetime `stats` totals are kept intact):

```sh
lowfat history prune                    # default: --older-than 90d
lowfat history prune --older-than 30d   # 30d, 2w, 3m accepted
lowfat history prune --below 2          # drop one-off commands
lowfat history prune --kept-by-plugin   # drop groups already handled by a plugin
lowfat history prune --all              # wipe all invocation rows
lowfat history prune --dry-run [...]    # preview without deleting
```
