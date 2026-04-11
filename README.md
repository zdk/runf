<p align="center">
  <img src="docs/lowfat_logo_v2.svg" alt="lowfat" width="500">
</p>

<p align="center">
  lowfat reduces AI token costs automatically by filtering unnecessary CLI output before it reaches your agent.
</p>

Wrap commands as shell functions and pipe them through composable processors like `grep`, `cut`, `head`, and `token-budget`.

<p align="center">
  <img src="docs/demo.gif" alt="lowfat demo: git diff before and after" width="700">
</p>

_Key features_,

- You own your data — customize or add your own filters in shell script.
- Composable, pipe-based processing stages.
- Per-project pipeline customization.
- Built-in secret redaction.
- No telemetry.

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

```sh
lowfat config             # show resolved config and validate .lowfat
lowfat filters            # list enabled/disabled filters
lowfat pipeline git       # show active pipeline for a command
lowfat gain               # show lifetime token savings report
lowfat audit              # show recent plugin executions
lowfat status             # show status badge
```

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

| Env var | Effect |
|---------|--------|
| `LOWFAT_LEVEL` | Override level (`lite`, `full`, `ultra`) |
| `LOWFAT_DISABLE` | Comma-separated filters to disable |
| `LOWFAT_HOME` | Plugin/config home (default: `~/.lowfat`) |
| `LOWFAT_DATA` | Data directory for history db (default: `~/.local/share/lowfat`) |

Env vars take priority over `.lowfat` file.

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

| Processor | Syntax | Description |
|-----------|--------|-------------|
| `grep` | `grep:pattern` | Keep lines matching regex |
| `grep-v` | `grep-v:pattern` | Remove lines matching regex |
| `head` | `head:N` | First N lines |
| `truncate` | `truncate:N` | First N characters per line |
| `cut` | `cut:1,3` or `cut:2-5` | Extract fields (`cut:,;1,3` for comma delimiter) |
| `strip-ansi` | `strip-ansi` | Remove ANSI escape codes |
| `token-budget` | `token-budget:N` | Trim to ~N tokens |
| `dedup-blank` | `dedup-blank` | Collapse consecutive blank lines |
| `normalize` | `normalize` | Trim whitespace, collapse blanks (runs automatically) |
| `redact-secrets` | `redact-secrets` | Mask API keys, tokens, passwords |

### Plugins

For command-specific filtering beyond built-in processors, plugins are shell scripts that read raw output from stdin and write filtered output to stdout.

Bundled plugins: `git-compact`, `docker-compact`, `ls-compact`, `npm-compact`, `go-compact`, `cargo-compact`

```sh
lowfat plugin list          # list installed plugins
lowfat plugin new cargo     # scaffold a new plugin
lowfat plugin bench cargo   # benchmark against sample files
lowfat plugin doctor        # check plugin health
```

`lowfat plugin new cargo` creates `~/.lowfat/plugins/cargo/cargo-compact/` with:

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

AI tools were used for this project
