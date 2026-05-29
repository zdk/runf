# OpenCode plugin

Thin TypeScript plugin that routes commands through lowfat for token savings.

- Install: `lowfat opencode install` → `~/.config/opencode/plugins/lowfat.ts`
- Mechanism: on `tool.execute.before` (bash/shell), calls `lowfat rewrite <cmd>`
  and swaps in the result; silently passes through on any failure.
- Single source of truth: rewrite logic lives in
  `crates/lowfat/src/commands/rewrite.rs`, not this file. The `.ts` is embedded
  in the binary via `include_str!`, so install needs no network or repo.
