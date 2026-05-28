#!/bin/sh
# cargo-compact — compact cargo output for LLM contexts
# stdin: raw command output
# env: LOWFAT_LEVEL, LOWFAT_SUBCOMMAND

RAW=$(cat)
LEVEL="${LOWFAT_LEVEL:-full}"
SUB="${LOWFAT_SUBCOMMAND}"

case "$SUB" in
  build|check)
    if [ "$LEVEL" = "ultra" ]; then
      # Errors and warnings only, no help text
      ISSUES=$(echo "$RAW" | grep -E '^(error|warning)\b' | head -n 15)
      if [ -z "$ISSUES" ]; then
        echo "cargo $SUB: ok"
      else
        echo "$ISSUES"
      fi
    else
      # Strip Compiling/Downloading/Checking noise, keep errors/warnings/Finished
      LIMIT=$( [ "$LEVEL" = "lite" ] && echo 60 || echo 30 )
      echo "$RAW" | grep -vE '^\s*(Compiling|Downloading|Checking|Blocking|Updating|Locking) ' | head -n "$LIMIT"
    fi
    ;;
  test)
    if [ "$LEVEL" = "ultra" ]; then
      # Test result summaries + failures only
      echo "$RAW" | grep -E '^(test result:|failures:|test .+ FAILED|     Running|FAILED)' | head -n 15
    else
      # Strip Compiling noise and individual ok tests
      LIMIT=$( [ "$LEVEL" = "lite" ] && echo 60 || echo 30 )
      echo "$RAW" | grep -vE '^\s*(Compiling|Downloading|Checking|Blocking|Updating|Locking) |\.\.\.+ ok$' | head -n "$LIMIT"
    fi
    ;;
  clippy)
    if [ "$LEVEL" = "ultra" ]; then
      # Just warning/error summary lines
      ISSUES=$(echo "$RAW" | grep -E '^(error|warning)\b' | grep -v 'generated' | head -n 15)
      SUMMARY=$(echo "$RAW" | grep -E 'warning.*generated')
      if [ -z "$ISSUES" ] && [ -z "$SUMMARY" ]; then
        echo "cargo clippy: ok"
      else
        [ -n "$ISSUES" ] && echo "$ISSUES"
        [ -n "$SUMMARY" ] && echo "$SUMMARY"
      fi
    else
      # Strip Checking noise, keep warnings/errors with context
      LIMIT=$( [ "$LEVEL" = "lite" ] && echo 60 || echo 30 )
      echo "$RAW" | grep -vE '^\s*(Compiling|Downloading|Checking|Blocking|Updating|Locking) ' | head -n "$LIMIT"
    fi
    ;;
  run)
    # Strip Compiling/Finished lines, show app output
    LIMIT=$( [ "$LEVEL" = "ultra" ] && echo 15 || { [ "$LEVEL" = "lite" ] && echo 60 || echo 30; })
    echo "$RAW" | grep -vE '^\s*(Compiling|Finished|Running) ' | head -n "$LIMIT"
    ;;
  add|update)
    if [ "$LEVEL" = "ultra" ]; then
      CHANGES=$(echo "$RAW" | grep -E '(Adding|Removing|Updating|Locking)')
      if [ -z "$CHANGES" ]; then
        echo "cargo $SUB: ok"
      else
        echo "$CHANGES" | head -n 10
      fi
    else
      echo "$RAW" | head -n 30
    fi
    ;;
  *)
    echo "$RAW" | head -n 30
    ;;
esac
