#!/bin/sh
# npm-compact — compact npm output for LLM contexts
# stdin: raw command output
# env: LOWFAT_LEVEL, LOWFAT_SUBCOMMAND

RAW=$(cat)
LEVEL="${LOWFAT_LEVEL:-full}"
SUB="${LOWFAT_SUBCOMMAND}"

case "$SUB" in
  install|i)
    if [ "$LEVEL" = "ultra" ]; then
      # Summary line only: "added 150 packages in 3s"
      SUMMARY=$(echo "$RAW" | grep -E '^added [0-9]+')
      echo "${SUMMARY:-npm install: ok}"
    else
      # Strip progress bars and warnings
      LIMIT=$( [ "$LEVEL" = "lite" ] && echo 60 || echo 30 )
      echo "$RAW" | grep -vE '^npm warn|░|█' | head -n "$LIMIT"
    fi
    ;;
  test|t)
    if [ "$LEVEL" = "ultra" ]; then
      # Keep only pass/fail summary and error lines
      echo "$RAW" | grep -E '^\s*(Tests|Test Suites|✓|✗|×|PASS|FAIL|Error)|^[0-9]+ (passing|failing|pending)' | head -n 10
    else
      LIMIT=$( [ "$LEVEL" = "lite" ] && echo 60 || echo 30 )
      echo "$RAW" | head -n "$LIMIT"
    fi
    ;;
  audit)
    if [ "$LEVEL" = "ultra" ]; then
      # Severity summary only
      echo "$RAW" | grep -iE '^[0-9]+ vulnerabilities|^found [0-9]+|low|moderate|high|critical' | head -n 5
    else
      LIMIT=$( [ "$LEVEL" = "lite" ] && echo 60 || echo 30 )
      echo "$RAW" | grep -v '^  fix available via' | head -n "$LIMIT"
    fi
    ;;
  run)
    # Strip "> script" echo lines
    case "$LEVEL" in
      ultra) LIMIT=15 ;; lite) LIMIT=60 ;; *) LIMIT=30 ;;
    esac
    echo "$RAW" | grep -v '^> ' | head -n "$LIMIT"
    ;;
  *)
    echo "$RAW" | head -n 30
    ;;
esac
