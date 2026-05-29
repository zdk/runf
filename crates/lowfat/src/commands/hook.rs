use anyhow::Result;
use serde_json::{json, Value};
use std::io::Read;

/// PreToolUse hook for Claude Code.
/// Reads hook JSON from stdin, rewrites Bash commands to pipe through lowfat.
pub fn run() -> Result<()> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;

    let payload: Value = serde_json::from_str(&input)?;

    let tool = payload["tool_name"].as_str().unwrap_or("");
    if tool != "Bash" {
        // Not a Bash call — pass through
        return Ok(());
    }

    let command = match payload["tool_input"]["command"].as_str() {
        Some(cmd) => cmd,
        None => return Ok(()),
    };

    // Reuse the canonical rewrite logic (shared with `lowfat rewrite` and the
    // OpenCode plugin). `None` means no filter applies — pass through.
    let rewritten = match crate::commands::rewrite::rewrite_command(command) {
        Some(r) => r,
        None => return Ok(()),
    };

    let output = json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "updatedInput": {
                "command": rewritten,
                "description": payload["tool_input"]["description"]
            }
        }
    });

    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    /// Simulate hook processing by extracting the rewrite logic. The hook
    /// rewrites a command iff a plugin (disk or embedded) claims it.
    fn rewrite_command(command: &str) -> Option<String> {
        let base_cmd = command.split_whitespace().next()?;
        if base_cmd == "lowfat" || base_cmd == "lf" {
            return None;
        }
        // Discover from a deliberately-empty disk dir so we exercise only the
        // embedded plugins (git/docker/ls). Real callers use the resolved
        // plugin_dir.
        let plugins = lowfat_plugin::discovery::discover_plugins(
            std::path::Path::new("/nonexistent-dir-for-test"),
        );
        let map = lowfat_plugin::discovery::resolve_plugins(&plugins);
        if map.contains_key(base_cmd) {
            Some(format!("lowfat {command}"))
        } else {
            None
        }
    }

    #[test]
    fn rewrites_git_command() {
        let result = rewrite_command("git status");
        assert_eq!(result, Some("lowfat git status".into()));
    }

    #[test]
    fn rewrites_docker_command() {
        let result = rewrite_command("docker ps");
        assert_eq!(result, Some("lowfat docker ps".into()));
    }

    #[test]
    fn skips_already_wrapped() {
        assert_eq!(rewrite_command("lowfat git status"), None);
    }

    #[test]
    fn skips_unknown_command() {
        assert_eq!(rewrite_command("curl https://example.com"), None);
    }
}
