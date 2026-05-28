use anyhow::Result;

/// Print shell init script for eval.
/// Usage: eval "$(lowfat shell-init zsh)"
///
/// The generated script queries `lowfat filters --commands` at eval time
/// to discover all available filters (builtins + plugins + pipelines),
/// then creates wrapper functions for each command.
pub fn run(shell: &str) -> Result<()> {
    match shell {
        "zsh" | "bash" => print_posix_init(),
        "fish" => print_fish_init(),
        _ => {
            eprintln!("lowfat: unsupported shell: {shell} (supported: bash, zsh, fish)");
        }
    }
    Ok(())
}

fn print_posix_init() {
    print!(r#"# lowfat shell init — auto-wrap commands for LLM token savings
# Usage: eval "$(lowfat shell-init zsh)"

if [[ "$CLAUDECODE" == "1" ]] || [[ -n "$CODEX_ENV" ]] || [[ "$LOWFAT_ENABLE" == "1" ]]; then
  for _lf_cmd in $(command lowfat filters --commands 2>/dev/null); do
    eval "$_lf_cmd() {{ command lowfat $_lf_cmd \"\$@\"; }}"
  done
  unset _lf_cmd
fi
"#);
}

fn print_fish_init() {
    print!(r#"# lowfat shell init for fish
# Usage: lowfat shell-init fish | source

if test "$CLAUDECODE" = 1; or test -n "$CODEX_ENV"; or test "$LOWFAT_ENABLE" = 1
  for cmd in (command lowfat filters --commands 2>/dev/null)
    eval "function $cmd; command lowfat $cmd \$argv; end"
  end
end
"#);
}
