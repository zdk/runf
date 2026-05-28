#!/bin/sh
# go-compact — compact go output for LLM contexts
# stdin: raw command output
# env: LOWFAT_LEVEL, LOWFAT_SUBCOMMAND

RAW=$(cat)
LEVEL="${LOWFAT_LEVEL:-full}"
SUB="${LOWFAT_SUBCOMMAND}"

case "$SUB" in
  build)
    if [ "$LEVEL" = "ultra" ]; then
      # Errors only
      ERRORS=$(echo "$RAW" | grep -E ': .*(error|undefined|cannot)')
      if [ -z "$ERRORS" ]; then
        echo "go build: ok"
      else
        echo "$ERRORS" | head -n 10
      fi
    else
      # Strip "# package" comment lines
      LIMIT=$( [ "$LEVEL" = "lite" ] && echo 60 || echo 30 )
      echo "$RAW" | grep -v '^# ' | head -n "$LIMIT"
    fi
    ;;
  test)
    if [ "$LEVEL" = "ultra" ]; then
      # Pass/fail summary + FAIL lines only
      echo "$RAW" | grep -E '^(ok |FAIL|---)' | head -n 15
    else
      # Strip verbose test noise (=== RUN, === PAUSE, === CONT, blank lines)
      LIMIT=$( [ "$LEVEL" = "lite" ] && echo 60 || echo 30 )
      echo "$RAW" | grep -vE '^\s*$|^=== (RUN|PAUSE|CONT)' | head -n "$LIMIT"
    fi
    ;;
  vet)
    if [ "$LEVEL" = "ultra" ]; then
      ISSUES=$(echo "$RAW" | grep -E ': ')
      if [ -z "$ISSUES" ]; then
        echo "go vet: ok"
      else
        echo "$ISSUES" | head -n 10
      fi
    else
      echo "$RAW" | head -n 30
    fi
    ;;
  mod)
    if [ "$LEVEL" = "ultra" ]; then
      # Keep only meaningful changes
      CHANGES=$(echo "$RAW" | grep -E '^go:|added|upgraded|removed')
      if [ -z "$CHANGES" ]; then
        echo "go mod: ok"
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
