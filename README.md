<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="./docs/lowfat_logo_dark.svg">
    <img src="./docs/lowfat_logo_light.svg" alt="lowfat logo" width="700">
  </picture>
</p>

lowfat is a lightweight CLI tool that reduces AI token costs by filtering unnecessary CLI output before it reaches your agent.

<p align="center">
  <img src="docs/demo.gif" alt="lowfat demo: git diff before and after" width="700">
</p>

### Core focus

- **Lightweight** — Small single binary, small core; but extensible.
- **Local-first** — No telemetry; you own your data.
- **Composable** — UNIX-style pipes, mix built-ins and your own filters; not magic.
- **User-owned** — `lowfat history` shows what you run most; allow you to customize for your usecase.

### Token savings on real commands

| Command        | Raw    | Filtered | Saved   |
| -------------- | ------ | -------- | ------- |
| `git status`   | 115t   | 5t       | **96%** |
| `git diff`     | 2,376t | 115t     | **95%** |
| `git log`      | 379t   | 118t     | **69%** |
| `docker ps`    | 271t   | 41t      | **85%** |
| `ls -la`       | 192t   | 30t      | **84%** |

### Install

```sh
cargo install lowfat
# or
brew install zdk/tools/lowfat
```

Pre-built binaries on [GitHub Releases](https://github.com/zdk/lowfat/releases).

### Setup

Pick one of:

**Claude Code hook** — add to `.claude/settings.json`:

```json
{
  "hooks": {
    "PreToolUse": [
      { "matcher": "Bash", "hooks": [{ "type": "command", "command": "lowfat hook" }] }
    ]
  }
}
```

**Shell integration** — auto-activates inside agent environments (`CLAUDECODE=1`, `CODEX_ENV`), or set `LOWFAT_ENABLE=1` to force it on any shell:

```sh
echo 'eval "$(lowfat shell-init zsh)"' >> ~/.zshrc   # or ~/.bashrc
```

**OpenCode plugin** — one command, no config editing:

```sh
lowfat opencode install   # writes ~/.config/opencode/plugins/lowfat.ts
```

Restart OpenCode; commands are rewritten transparently before they run.
Remove it anytime with `lowfat opencode uninstall`.

**Direct usage** — prefix any command:

```sh
lowfat git status
lowfat docker ps
lowfat ls -la
```

**pi agent** — in `~/.pi/agent/settings.json`:

```json
{ "shellCommandPrefix": "eval \"$(lowfat shell-init zsh)\"; " }
```

### Usage highlights

```sh
# See what's configured and how loud each filter is being
lowfat info                       # status badge + active filters
lowfat info git                   # pipeline for `git`
lowfat info --config              # full resolved config

# See what lowfat has saved you
lowfat stats                      # lifetime token savings
lowfat stats --audit              # recent plugin executions
lowfat history                    # rank commands by potential savings

# Dial the aggressiveness
lowfat level ultra                # max compression
LOWFAT_LEVEL=lite lowfat git log  # one-off override

# Write a plugin
lowfat plugin new terraform       # scaffold ~/.lowfat/plugins/terraform/
lowfat plugin doctor              # check plugins (and pre-install any Python deps)

# Test a plugin against a sample without installing it
cat samples/git-diff-full.txt | lowfat filter --explain ./filter.lf --sub=diff --level=ultra
```

### Learn more

- **[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)** — high-level diagram: CLI, Runner, Plugins, Builtins
- **[docs/CONFIG.md](docs/CONFIG.md)** — `.lowfat` file, env vars, pipeline DSL, built-in processors, the `history` ranking
- **[docs/PLUGINS.md](docs/PLUGINS.md)** — lf-filter (the `.lf` plugin DSL), shell escape hatches, PEP 723 + uv, AI agent prompt

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
