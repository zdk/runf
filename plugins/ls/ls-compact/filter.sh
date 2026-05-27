#!/bin/sh
# ls-compact — compact ls output for LLM contexts
# env: $LOWFAT_LEVEL (lite|full|ultra)

RAW=$(cat)
LEVEL="${LOWFAT_LEVEL:-full}"

case "$LEVEL" in
  ultra)
    # Filenames only
    echo "$RAW" | grep -v '^total ' | grep -v '^$' | awk '{print $NF}' | head -n 40
    ;;
  lite)
    # Gentle trim — preserve long-form metadata
    echo "$RAW" | grep -v '^total ' | grep -v '^$' | head -n 40
    ;;
  *)
    # Strip "total" line; compact `ls -l` long-form to `<type> <size> <name>`
    echo "$RAW" | grep -v '^total ' | grep -v '^$' | awk '
      NF >= 9 && $1 ~ /^[-dlbcps][-r]/ {
          t = substr($1,1,1); s = $5;
          name = $9; for (i=10; i<=NF; i++) name = name " " $i
          print t, s, name; next
      }
      { print }
    ' | head -n 40
    ;;
esac
