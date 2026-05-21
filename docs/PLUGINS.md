# Writing a lowfat plugin

Most commands can be filtered with a one-liner in `.lowfat` (see [CONFIG.md](CONFIG.md#filtering-any-command-without-writing-a-plugin)). Write a plugin only when pipeline config can't express what you need:

- **Per-subcommand logic** — different subcommands produce completely different output
- **Conditional output** — show "ok" when clean, show errors when not
- **Context-aware filtering** — different behavior based on flags or arguments

A plugin lives at `~/.lowfat/plugins/<category>/<name>/` and ships in one of two formats:

| Format          | When to use                                                                                                                                                                |
| --------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **`filter.lf`** | Default for new plugins. Declarative **lf-filter** rules parsed in-process; shell + python escape hatches for the rare cases that can't be expressed in built-in ops.      |
| **`filter.sh`** | Legacy. POSIX shell script reading stdin → writing stdout. Still supported — when both files exist `filter.lf` is auto-detected; set `runtime.entry` to force `filter.sh`. |

## Quick start

```sh
lowfat plugin new kubectl    # scaffold at ~/.lowfat/plugins/kubectl/kubectl-compact/
```

This creates:

```
~/.lowfat/plugins/kubectl/kubectl-compact/
  lowfat.toml     # manifest
  filter.lf       # rule file (edit this)
  samples/        # paste real output here for benchmarking
```

Edit the manifest:

```toml
[plugin]
name = "kubectl-compact"
commands = ["kubectl"]
subcommands = ["get", "describe", "logs", "apply"]
```

- `commands` — top-level command(s) intercepted (e.g., `kubectl`)
- `subcommands` — which subcommands this plugin handles (omit to handle all)
- the entrypoint is auto-detected (`filter.lf`, else `filter.sh`) — add a `[runtime]` table only to override it or declare `requires`

---

# The lf-filter DSL

**lf-filter** is lowfat's declarative plugin DSL. A `.lf` file is a sequence of `define` blocks and `rule` blocks; the runner picks the first rule whose selector matches `(subcommand, level)` and runs its ops top-to-bottom.

## Selectors

```awk
status:                        # any level
build|check, ultra:            # subcommand alternation, specific level
*, ultra:                      # any subcommand, ultra only
*:                             # catch-all (put last)
```

First match wins. Order matters: put specific rules before catch-alls.

## Ops (built-in, run in-process)

| Op            | Form                                      | What it does                                                         |
| ------------- | ----------------------------------------- | -------------------------------------------------------------------- |
| `keep`        | `keep /regex/`                            | Keep lines matching the regex                                        |
| `drop`        | `drop /regex/`                            | Drop lines matching the regex                                        |
| `head`        | `head N` or `head auto`                   | First N lines (`auto` = level-scaled: 15/30/60 for ultra/full/lite)  |
| `tail`        | `tail N` or `tail auto`                   | Last N lines                                                         |
| `else`        | `else "text"`                             | If state is empty, emit literal `text`                               |
| `else-shell:` | `else-shell: <cmd>`                       | If state is empty, run `<cmd>` with the **original** raw input       |
| `split`       | `split /regex/` + `pre:` / `post:` blocks | Split input at first matching line, run separate chains on each half |

Regex uses `/.../` delimiters (escape `/` as `\/`). Built-in ops are pure Rust — no subprocess overhead.

Ops run top-to-bottom as a pipeline — each one receives the previous op's output. Combinations compose by intersection: `keep /error/` then `drop /ignored/` keeps lines matching `error` AND not matching `ignored`. Order matters: `keep /X/` then `drop /X/` produces nothing.

## Escape hatches (subprocess)

| Op        | Form                                              | Notes                                                                                            |
| --------- | ------------------------------------------------- | ------------------------------------------------------------------------------------------------ |
| `shell:`  | `shell: <inline>` or `shell: \|` + indented block | Runs under `sh -c`. Env: `$level`, `$sub`, `$args`, `$exit`                                      |
| `python:` | `python: \|` + indented block                     | `python3 -c` for plain bodies; `uv run --script` for bodies with a `# /// script` PEP 723 header |

## Macros: `define`

Reusable op sequences with positional arguments:

```awk
define strip-trailers:
    drop /^[[:space:]]*(Signed-off-by|Co-authored-by):/

define compact-diff(limit):
    shell: |
        awk -v lim=$1 -v lvl=$level '
          BEGIN { in_hunk=0; n=0 }
          n>=lim { exit }
          /^diff / { in_hunk=0; print; n++; next }
          /^@@ /  { in_hunk=1; print; n++; next }
          in_hunk && /^[+-]/ { print; n++ }
        '
```

`$1`, `$2`, … are substituted at execution time. `$level`, `$sub`, `$args`, `$exit` are left for the shell to expand from env vars.

Invocation:

```awk
diff, ultra:    compact-diff 30
diff, lite:     compact-diff 400
diff:           compact-diff 200
strip-trailers              # zero-arg call
```

## Inline ops on a rule header

```awk
diff, ultra:    compact-diff 30     else-shell: awk 'NF' | head -50
```

`shell:` / `python:` / `else-shell:` greedily consume the rest of the line.

## Worked example: `git-compact.lf`

```awk
#!/usr/bin/env lowfat-filter

define strip-trailers:
    drop /^[[:space:]]*(Signed-off-by|Co-authored-by|Change-Id|Reviewed-by|Acked-by|Tested-by|Reported-by|Cc):/

define abbrev-hash:
    shell: sed -E 's/^commit ([0-9a-f]{12})[0-9a-f]{28}/commit \1/'

define compact-diff(limit):
    shell: |
        awk -v lim=$1 -v lvl=$level '
          BEGIN { in_hunk=0; n=0 }
          n>=lim { exit }
          /^diff / { in_hunk=0; print; n++; next }
          /^@@ /  { in_hunk=1
                    if (lvl=="ultra" && match($0,/ @@/))
                        print substr($0,1,RSTART+2)
                    else print
                    n++; next }
          lvl=="ultra" { next }
          in_hunk && /^[+-]/ { print; n++ }
        '

status:
    keep /^\s*[MADRCU?!] /
    head 30
    else "git status: clean"

diff, ultra:    compact-diff 30     else-shell: awk 'NF' | head -50
diff, lite:     compact-diff 400    else-shell: awk 'NF' | head -50
diff:           compact-diff 200    else-shell: awk 'NF' | head -50

log, ultra:
    keep /^(commit |    )/
    strip-trailers
    abbrev-hash
    head 10

log:
    strip-trailers
    abbrev-hash
    head 25

show:
    split /^diff /
    pre:
        keep /^(commit |Merge:|Author:|Date:|    )/
        strip-trailers
        abbrev-hash
    post:
        compact-diff 100
    head 100

*:
    head 30
```

54 lines, down from 134 in the original shell version. The shell escape hatches handle the genuinely stateful work (diff hunk machine, sed rewrite); everything else is declarative.

## Worked example: `kubectl-compact.lf` (Python + uv)

`kubectl get -o yaml` dumps server-side fields (`managedFields`, `resourceVersion`, `generation`, `creationTimestamp`) that drown the manifest. Real YAML parsing beats regex — annotations can contain anything including embedded `---`.

```awk
#!/usr/bin/env lowfat-filter

define clean-yaml:
    python: |
        # /// script
        # requires-python = ">=3.10"
        # dependencies = ["pyyaml>=6"]
        # ///
        import sys, yaml

        DROP = {"managedFields", "resourceVersion", "generation",
                "creationTimestamp", "uid", "selfLink",
                "ownerReferences"}

        def prune(node):
            if isinstance(node, dict):
                if "annotations" in node:
                    node["annotations"] = f"<{len(node['annotations'])} entries>"
                return {k: prune(v) for k, v in node.items() if k not in DROP}
            if isinstance(node, list):
                return [prune(x) for x in node]
            return node

        raw = sys.stdin.read()
        try:
            docs = list(yaml.safe_load_all(raw))
        except yaml.YAMLError:
            sys.stdout.write(raw)            # passthrough non-YAML
            sys.exit(0)

        for d in docs:
            if d is None: continue
            yaml.safe_dump(prune(d), sys.stdout, default_flow_style=False, sort_keys=False)
            print("---")

get:
    clean-yaml
    head 200

logs, ultra:
    keep /ERROR|FATAL|panic|Exception/
    tail 30
    else "no errors in window"

logs:
    drop /^\s*$/
    tail 60

events, ultra:
    keep /Warning|Error/
    tail 20

events:
    tail 40

*:
    head 30
```

Declare uv as a dep in `lowfat.toml`:

```toml
[runtime.requires]
python = ">=3.10"
uv = "*"
```

`lowfat plugin doctor` detects the `# /// script` header and prewarms the uv env (~2 s first time, then cached). Subsequent runs hit a warm cache (~200 ms overhead). Use Python for heavy commands (`kubectl describe`, `terraform plan`); stay with shell/awk for hot-path commands (`git status`).

## Filter contract

| Level        | What to emit                                                           |
| ------------ | ---------------------------------------------------------------------- |
| `ultra`      | Verdict line(s) only — what the AI needs to decide next                |
| `full`       | Strip progress chatter / banner prose; keep diffs / errors / structure |
| `lite`       | Gentle trim, higher row caps                                           |
| `$exit != 0` | Be conservative — preserve error blocks                                |

`$sub` is the first arg of the original command (`get`, `describe`, …). Walk `$args` when the subcommand alone isn't enough (resource type, output flags). Empty output = passthrough (lowfat falls back to the original).

---

# Testing

## Standalone with `lowfat filter`

```sh
# Run a sample through the filter
cat samples/git-diff-full.txt | lowfat filter ./filter.lf --sub=diff --level=ultra

# Inspect per-stage cost (line/byte/token counts to stderr; output still to stdout)
cat samples/git-diff-full.txt | lowfat filter --explain ./filter.lf --sub=diff --level=ultra

# Side-by-side comparison
diff -u \
  <(lowfat filter ./filter.lf --sub=diff --level=full < raw.diff) \
  <(LOWFAT_LEVEL=full LOWFAT_SUBCOMMAND=diff sh ./filter.sh < raw.diff)
```

## Benchmark against captured samples

```sh
# Naming convention: <command>-<subcommand>-<level>.txt
kubectl get pods -A > samples/kubectl-get-full.txt
kubectl describe pod mypod > samples/kubectl-describe-full.txt

lowfat plugin bench kubectl-compact
```

Aim for **80%+ savings** at `full` on noisy commands, while keeping all actionable information.

## Plugin health

```sh
lowfat plugin doctor
# checks: manifest parses, .lf parses, uv installed if needed,
#         pre-resolves PEP 723 envs so first real run is fast
```

---

# Writing as `filter.sh` (shell)

The script reads stdin, writes stdout, and gets these env vars:

| Env var              | Value                     | Example (`lowfat kubectl get pods -n kube-system`) |
| -------------------- | ------------------------- | -------------------------------------------------- |
| `$LOWFAT_COMMAND`    | top-level command         | `kubectl`                                          |
| `$LOWFAT_SUBCOMMAND` | first argument            | `get`                                              |
| `$LOWFAT_ARGS`       | all arguments joined      | `get pods -n kube-system`                          |
| `$LOWFAT_LEVEL`      | `lite` / `full` / `ultra` | `full`                                             |
| `$LOWFAT_EXIT_CODE`  | command's exit code       | `0`                                                |

The recurring shape:

```sh
#!/bin/sh
RAW=$(cat)
LEVEL="${LOWFAT_LEVEL:-full}"
SUB="${LOWFAT_SUBCOMMAND}"

case "$SUB" in
  <subcommand>)
    if [ "$LEVEL" = "ultra" ]; then
      # Extract summary/errors only
    else
      LIMIT=$( [ "$LEVEL" = "lite" ] && echo 60 || echo 30 )
      echo "$RAW" | grep -vE '<noise patterns>' | head -n "$LIMIT"
    fi
    ;;
  *)
    echo "$RAW" | head -n 30
    ;;
esac
```

---

# Advanced

## Pipeline integration

Mix your plugin with built-in processors in `.lowfat`:

```
pipeline.kubectl = strip-ansi | kubectl-compact | truncate:100
pipeline.kubectl.error = strip-ansi | head
```

## Manifest options

```toml
[plugin]
name = "kubectl-compact"
version = "0.1.0"
description = "Compact kubectl output"
author = "you"
commands = ["kubectl"]
subcommands = ["get", "describe", "logs", "apply"]

[runtime]
entry = "filter.lf"       # optional — auto-detected (filter.lf, else filter.sh)

[runtime.requires]        # checked by `lowfat plugin doctor`
python = ">=3.10"
uv = "*"

[hooks]
on_install = "chmod +x filter.sh"

[pipeline]
pre = ["strip-ansi"]      # run before your filter
post = ["truncate"]       # run after your filter
```

---

# Building a plugin with an AI agent

Copy-paste this into Claude Code (or another tool-using agent) and replace `<COMMAND>`:

```
Create a lowfat plugin to filter `<COMMAND>` output for LLM contexts.

Before writing code:
1. Read docs/PLUGINS.md to learn lf-filter, lowfat's plugin DSL: ops
   (keep/drop/head/tail/else, shell:, python:), selectors (sub, level),
   define macros, split.
2. Ask me: which subcommands to specialize, and what's noise vs. signal
   in this command's output.

Scaffold at `~/.lowfat/plugins/<COMMAND>/<COMMAND>-compact/`:
- `lowfat.toml` — manifest with `commands` (any aliases too) and the agreed
  `subcommands` list (no `[runtime]` needed — `filter.lf` is auto-detected)
- `filter.lf` — rules, top-down first-match; reach for `shell:` / `python:`
  only when keep/drop/head can't express it

Filter contract:
- level=ultra → verdict line(s) only
- level=full  → strip noise (progress chatter, banner prose), keep diffs /
                errors / structure
- level=lite  → gentle trim, higher row caps
- Non-zero $exit → be conservative; preserve error blocks
- Use $sub, fall back to walking $args when you need flags or resource type

Verify:
- `lowfat plugin doctor`            (parses cleanly; prewarms uv envs)
- Drop real captures in `samples/<command>-<sub>-full.txt`
- `cat samples/foo.txt | lowfat filter --explain filter.lf --sub=... --level=...`
- `lowfat plugin bench <name>`      (aim ≥80% at full on noisy commands)
- Smoke-test: `lowfat <COMMAND> ...` against a real run
```

The agent will ask you what counts as noise — answer with sample output if you have it, then iterate on the bench numbers.
