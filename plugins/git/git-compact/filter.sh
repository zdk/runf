#!/bin/sh
# git-compact — compact git output for LLM contexts.
# env: $LOWFAT_LEVEL (lite|full|ultra), $LOWFAT_SUBCOMMAND
#
# Reference implementation. The shipped binary uses the equivalent native
# filter at crates/lowfat/src/filters/git.rs — keep both in sync so bench.sh
# numbers track real behaviour.

RAW=$(cat)
LEVEL="${LOWFAT_LEVEL:-full}"
SUB="$LOWFAT_SUBCOMMAND"

# Drops three categories of redundancy:
#   - pre-hunk metadata (`--- a/X`, `+++ b/X`, `index …`, mode lines) — the
#     `--- ` / `+++ ` lines always duplicate the path on `diff --git`;
#   - unchanged context lines (` ` prefix) — only +/- carry the change;
#   - the `@@ … @@ <fn>` tail in ultra mode — function context is only kept
#     in lite/full where the LLM benefits from it.
# State machine tracks `in_hunk` so a removed source line that happens to
# start with `--- ` (e.g. comment delimiters) isn't misread as the header.
compact_diff_body() {
  level="$1"
  limit="$2"
  awk -v level="$level" -v limit="$limit" '
    BEGIN { in_hunk = 0; n = 0 }
    {
      if (n >= limit) exit
      if (index($0, "diff ") == 1) { in_hunk = 0; print; n++; next }
      if (index($0, "@@ ") == 1) {
        in_hunk = 1
        if (level == "ultra") {
          # Strip trailing function-context tail: `@@ -A,B +C,D @@ ctx` → `@@ -A,B +C,D @@`
          if (match($0, / @@/)) print substr($0, 1, RSTART + 2)
          else print
        } else print
        n++
        next
      }
      if (level == "ultra") next
      if (!in_hunk) next
      first = substr($0, 1, 1)
      if (first == "+" || first == "-") { print; n++ }
    }
  '
}

# Trailers add no signal for code understanding (DCO repos and pair-programming
# bots can pile up noticeably). Detect anywhere in body indentation.
strip_trailers() {
  grep -vE '^[[:space:]]*(Signed-off-by|Co-authored-by|Change-Id|Reviewed-by|Acked-by|Tested-by|Reported-by|Cc):'
}

# `commit <40-hex>[ decoration]` → `commit <12-hex>[ decoration]`.
# Decoration like `(HEAD -> main)` from `--decorate` is preserved.
abbreviate_commit_hash() {
  sed -E 's/^commit ([0-9a-f]{12})[0-9a-f]{28}/commit \1/'
}

case "$SUB" in
  status)
    # File entries: long-format indents with a tab; short/porcelain (-s)
    # prefixes two status-code columns. Full/lite also keep section headers
    # ("On branch", "Changes …", "Untracked", "## branch") for staged-vs-
    # unstaged context; ultra strips to file entries only.
    case "$LEVEL" in
      ultra) result=$(echo "$RAW" | grep -E '^(	|[ MADRCU?!]{2} )' | head -n 15) ;;
      lite)  result=$(echo "$RAW" | grep -E '^(	|[ MADRCU?!]{2} |## |On branch|Changes|Untracked)' | head -n 60) ;;
      *)     result=$(echo "$RAW" | grep -E '^(	|[ MADRCU?!]{2} |## |On branch|Changes|Untracked)' | head -n 30) ;;
    esac
    if [ -z "$result" ]; then
      echo "git status: clean"
    else
      echo "$result"
    fi
    ;;

  diff)
    case "$LEVEL" in
      lite)  body=$(echo "$RAW" | compact_diff_body lite 400)  ;;
      ultra) body=$(echo "$RAW" | compact_diff_body ultra 30)  ;;
      *)     body=$(echo "$RAW" | compact_diff_body full 200)  ;;
    esac
    if [ -z "$body" ]; then
      # No diff/@@ markers — likely --stat / --name-only / --shortstat.
      # Compact pass instead of empty-passthrough so we still record savings.
      echo "$RAW" | awk 'NF' | head -n 50
    else
      echo "$body"
    fi
    ;;

  log)
    case "$LEVEL" in
      ultra)
        echo "$RAW" | grep -E '^(commit |    )' | strip_trailers | abbreviate_commit_hash | head -n 10
        ;;
      lite)
        echo "$RAW" | strip_trailers | abbreviate_commit_hash | head -n 50
        ;;
      *)
        echo "$RAW" | strip_trailers | abbreviate_commit_hash | head -n 25
        ;;
    esac
    ;;

  show)
    case "$LEVEL" in
      ultra)
        # Commit metadata + diffstat only.
        echo "$RAW" \
          | grep -E '^(commit |Author:|Date:|    |diff --git)' \
          | strip_trailers \
          | abbreviate_commit_hash \
          | head -n 20
        ;;
      lite)
        # Permissive: drop only commit-message trailers and pre-hunk index/mode
        # meta (--- a/X / +++ b/X always duplicate the diff --git path).
        echo "$RAW" \
          | strip_trailers \
          | abbreviate_commit_hash \
          | grep -vE '^(index [0-9a-f]+\.\.[0-9a-f]+( [0-7]+)?|new file mode |deleted file mode |old mode |new mode |similarity index |dissimilarity index |rename from |rename to |copy from |copy to |---|\+\+\+) ' \
          | head -n 200
        ;;
      *)
        # Full/lite: split into pre-diff (commit metadata) and post-diff (hunks).
        # Pre-diff: keep commit headers, drop trailers, abbreviate the long hash.
        # Post-diff: hand off to compact_diff_body so we get the same metadata
        # drops as `git diff` (--- / +++ / index / mode redundancy).
        if echo "$RAW" | grep -q '^diff '; then
          pre=$(echo "$RAW" | awk '/^diff / { exit } { print }' \
            | grep -E '^(commit |Merge:|Author:|Date:|    )' \
            | strip_trailers \
            | abbreviate_commit_hash)
          post=$(echo "$RAW" | awk '/^diff / { found=1 } found { print }' \
            | compact_diff_body full 100)
          { [ -n "$pre" ] && echo "$pre"; [ -n "$post" ] && echo "$post"; } | head -n 100
        else
          # No diff content (e.g. `git show <tag>`) — commit-style output only.
          echo "$RAW" \
            | grep -E '^(commit |Merge:|Author:|Date:|    )' \
            | strip_trailers \
            | abbreviate_commit_hash \
            | head -n 60
        fi
        ;;
    esac
    ;;

  *)
    echo "$RAW" | head -n 30
    ;;
esac
