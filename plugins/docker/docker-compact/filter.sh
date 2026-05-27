#!/bin/sh
# docker-compact — compact docker output for LLM contexts
# env: $LOWFAT_LEVEL (lite|full|ultra), $LOWFAT_SUBCOMMAND

RAW=$(cat)
LEVEL="${LOWFAT_LEVEL:-full}"
SUB="$LOWFAT_SUBCOMMAND"

# Collapse multi-space columns into single space
collapse() {
  sed 's/  */ /g'
}

case "$SUB" in
  ps)
    case "$LEVEL" in
      ultra)
        # Name + status only
        echo "NAME STATUS"
        echo "$RAW" | tail -n +2 | awk '{print $NF, $(NF-2)}' | head -n 20
        ;;
      lite)
        echo "$RAW" | collapse | head -n 80
        ;;
      *)
        echo "$RAW" | collapse | head -n 40
        ;;
    esac
    ;;

  images)
    case "$LEVEL" in
      ultra)
        # Repo + tag + size
        echo "REPO TAG SIZE"
        echo "$RAW" | tail -n +2 | awk '{print $1, $2, $(NF-1)}' | head -n 20
        ;;
      lite)
        echo "$RAW" | collapse | head -n 80
        ;;
      *)
        echo "$RAW" | collapse | head -n 40
        ;;
    esac
    ;;

  logs)
    case "$LEVEL" in
      ultra)  echo "$RAW" | tail -n 10 ;;
      full)   echo "$RAW" | tail -n 30 ;;
      *)      echo "$RAW" | tail -n 60 ;;
    esac
    ;;

  build)
    case "$LEVEL" in
      ultra)
        # Result lines only
        result=$(echo "$RAW" | grep -E '^(Successfully|ERROR|ERRO)' | tail -n 3)
        echo "${result:-docker build: ok}"
        ;;
      *)
        # Strip cache noise
        echo "$RAW" | grep -v -E '^(#[0-9]+ (CACHED|sha256:)|--->) ' | grep -v '^$' | tail -n 50
        ;;
    esac
    ;;

  pull)
    case "$LEVEL" in
      ultra)
        # Status + digest only
        result=$(echo "$RAW" | grep -E '^(Status:|Digest:)' | tail -n 2)
        echo "${result:-docker pull: ok}"
        ;;
      *)
        # Strip layer progress
        echo "$RAW" | grep -v -E '^[0-9a-f]+: (Pulling|Waiting|Downloading|Extracting|Verifying|Pull complete)' | head -n 10
        ;;
    esac
    ;;

  compose)
    echo "$RAW" | grep -v -E '^(Pulling|Creating|Starting|Waiting) ' | grep -v '^$' | head -n 30
    ;;

  *)
    echo "$RAW" | head -n 40
    ;;
esac
