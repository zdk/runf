import type { Plugin } from "@opencode-ai/plugin"

// lowfat OpenCode plugin — rewrites commands to run through lowfat for LLM
// token savings. Requires the `lowfat` binary in PATH.
//
// Thin delegating plugin: all rewrite logic lives in `lowfat rewrite`, the
// single source of truth (crates/lowfat/src/commands/rewrite.rs). To change
// which commands get wrapped, edit lowfat — not this file.

export const LowfatOpenCodePlugin: Plugin = async ({ $ }) => {
  try {
    await $`which lowfat`.quiet()
  } catch {
    console.warn("[lowfat] lowfat binary not found in PATH — plugin disabled")
    return {}
  }

  return {
    "tool.execute.before": async (input, output) => {
      const tool = String(input?.tool ?? "").toLowerCase()
      if (tool !== "bash" && tool !== "shell") return

      const args = output?.args
      if (!args || typeof args !== "object") return

      const command = (args as Record<string, unknown>).command
      if (typeof command !== "string" || !command) return

      try {
        const result = await $`lowfat rewrite ${command}`.quiet().nothrow()
        const rewritten = String(result.stdout).trim()
        if (rewritten && rewritten !== command) {
          ;(args as Record<string, unknown>).command = rewritten
        }
      } catch {
        // lowfat rewrite failed — pass through unchanged
      }
    },
  }
}
