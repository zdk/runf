# lf-filter v2 — guarded rules

Design spec for lowfat **v0.5.0**. Status: **implemented** — [PLUGINS.md](PLUGINS.md)
is the authoritative reference; this file is kept as the design record.

## What changes

v1 dispatches a rule on `(subcommand, level)` only. There is no declarative
way to branch on exit code or flags — the filter contract asks plugins to "be
conservative on non-zero exit," yet the only way to read `$exit` is a `shell:`
block, which defeats the DSL.

v2 adds **one** thing: a rule body can branch with an `if` / `elif` / `else`
**cascade**. Guards are a closed vocabulary — `exit`, `level`, flags. No
expressions, no variables, no loops. It stays a match table, not a programming
language; `shell:` / `python:` remain the escape hatch for real compute.

v2 is a **superset of v1** — every existing `.lf` file still parses.

## Rule structure

A rule is `selector:` followed by a body. A body is one of:

- a **pipeline** — ops run top-to-bottom, each transforming the stream (v1)
- a **cascade** — `if` / `elif` / `else` arms; the first arm whose guard
  matches runs, and only that one

A body is one or the other — never mixed.

```awk
# pipeline body
status:
    keep /^\s*[MADRCU?!] /
    or "working tree clean"
    head 30

# cascade body
diff:
    if exit failed:    raw
    elif level ultra:  compact 30
    elif --stat:       compact 40
    else:              compact 200
```

An arm's body is itself a pipeline — inline after the colon for a single op,
or indented for several:

```awk
log:
    if level ultra:
        keep /^(commit |    )/
        head 10
    else:
        head 25
```

`elif` and `else` are optional; any number of `elif`. **First matching guard
wins, exactly one arm runs.** If no arm matches and there is no `else`, the
stream passes through unchanged.

## Guards

A guard names a **dimension** and a **value** — both from closed sets, in a
fixed shape. It is not an expression and cannot become one.

| Guard | True when |
| ------------------- | ------------------------------------------ |
| `exit failed`       | exit code ≠ 0                              |
| `exit ok`           | exit code = 0                              |
| `level ultra`       | active level is ultra                      |
| `level full`        | active level is full                       |
| `level lite`        | active level is lite                       |
| `--stat`, `-p`, …   | that flag is present in the command's args |

`exit` and `level` are the same words already exposed to `shell:` blocks as
`$exit` / `$level` — one vocabulary throughout. A flag is recognised by its
leading `-` / `--`; it needs no dimension word.

Join guards in one arm with `and`:

```awk
if level ultra and --stat:    compact 20
```

For "or", write separate arms — first-match-wins already gives you the
disjunction. There is deliberately **no `or` operator, no comparisons, no
parentheses**: a guard cannot grow into an expression. That closedness is what
keeps lf-filter analyzable and out of general-purpose-language territory — and
it is why a guard reads the same to a newcomer as to `lowfat plugin doctor`.

## Ops

Unchanged from v1: `keep` `drop` `head` `tail` `split` `shell:` `python:`, and
macro calls. Two changes:

- **New — `raw`**: emit the stream unchanged. The conservative arm:
  `if exit failed: raw`. (`passthrough` is accepted as a legacy alias.)
- **Renamed — `else "text"` → `or "text"`** (and `else-shell:` → `or-shell:`),
  because `else` is now the cascade default arm. This op fires when the stream
  filtered down to *empty* — a stream-derived condition, distinct from the
  run-context guards above. v1's `else "..."` is still accepted as a legacy
  alias so old files keep working.

## Selectors

The subcommand token gains a `*` glob — `apply*:` matches `apply`, `apply-set`.
Alternation (`|`) and the catch-all `*:` are unchanged. v1's header level
(`diff, ultra:`) still parses, but the idiomatic home for level is now a guard.

## Worked example

```awk
# git-compact · lf-filter

define compact(limit):
    keep /^(diff |@@ |[+-])/
    head limit

status:
    keep /^\s*[MADRCU?!] /
    or "working tree clean"
    head 30

diff:
    if exit failed:    raw
    elif level ultra:  compact 30
    elif --stat:       compact 40
    else:              compact 200

log:
    if level ultra:
        keep /^(commit |    )/
        head 10
    else:
        head 25

*:
    head 30
```

Every line reads without the docs: `exit` / `level` name what is tested, a
flag carries its own `--`, and `if` / `elif` / `else` make "checked in order,
one runs" self-evident.

## Back-compat

| v1 construct                              | v2 status                          |
| ----------------------------------------- | ---------------------------------- |
| `status:` + pipeline ops                  | unchanged                          |
| `diff, ultra:` header level               | still parses; guard is idiomatic   |
| `else "text"` / `else-shell:`             | legacy alias of `or` / `or-shell:` |
| `define`, `$1` substitution               | unchanged                          |
| `keep` `drop` `head` `tail` `split` `shell:` `python:` | unchanged             |

## Implementation stages

| Stage | Work                                                                  |
| ----- | --------------------------------------------------------------------- |
| 1     | AST — `Op::Cascade(Vec<Branch>)`, `Op::Raw`, `Guard` / `Atom`          |
| 2     | Parser — `if` / `elif` / `else`, guard parsing, glob selectors        |
| 3     | Executor — cascade dispatch, guard eval, `raw`                        |
| 4     | Rename `else` → `or` / `else-shell:` → `or-shell:` (keep legacy alias) |
| 5     | `execute_explain` / `describe_op` cover the new ops                   |
| 6     | Migrate shipped plugins (git-compact, kubectl-compact)                |
| 7     | Rewrite the PLUGINS.md DSL section; bump workspace to v0.5.0           |
