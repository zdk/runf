#!/bin/sh
# Token saving benchmark for go-compact
# Usage: sh bench.sh

PLUGIN_DIR="$(cd "$(dirname "$0")" && pwd)"
FILTER="$PLUGIN_DIR/filter.sh"
SAMPLES_DIR="$PLUGIN_DIR/samples"

estimate_tokens() { echo $(( ($(printf '%s' "$1" | wc -c) + 3) / 4 )); }

printf "=== go-compact — token saving benchmark ===\n\n"
printf "%-35s %6s %6s %6s %8s\n" "SAMPLE" "LEVEL" "INPUT" "OUTPUT" "SAVED"
printf "%-35s %6s %6s %6s %8s\n" "-----------------------------------" "------" "------" "------" "--------"

for sample in "$SAMPLES_DIR"/*.txt; do
  [ -f "$sample" ] || continue
  name=$(basename "$sample" .txt)
  raw=$(cat "$sample")
  in_tokens=$(estimate_tokens "$raw")

  # Extract subcommand from filename: go-test-full → test
  sub=$(echo "$name" | sed 's/^go-//' | sed 's/-[a-z]*$//')

  for level in full ultra; do
    filtered=$(LOWFAT_SUBCOMMAND="$sub" LOWFAT_LEVEL="$level" sh "$FILTER" < "$sample")
    out_tokens=$(estimate_tokens "$filtered")
    saved=$(( in_tokens - out_tokens ))
    if [ "$in_tokens" -gt 0 ]; then
      pct=$(( saved * 100 / in_tokens ))
    else
      pct=0
    fi
    printf "%-35s %6s %5dt %5dt %5dt %3d%%\n" "$name" "$level" "$in_tokens" "$out_tokens" "$saved" "$pct"
  done
done
