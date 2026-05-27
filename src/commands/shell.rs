use anyhow::{bail, Result};
use clap::{CommandFactory, ValueEnum};
use clap_complete::Shell as CompletionShell;
use std::io;
use std::path::Path;

#[derive(Copy, Clone, Debug, ValueEnum, PartialEq, Eq)]
pub enum HookShell {
    Bash,
    Zsh,
}

impl HookShell {
    fn from_env() -> Option<Self> {
        let shell = std::env::var("SHELL").ok()?;
        let base = std::path::Path::new(&shell)
            .file_name()
            .and_then(|s| s.to_str())?;
        match base {
            "bash" => Some(HookShell::Bash),
            "zsh" => Some(HookShell::Zsh),
            _ => None,
        }
    }
}

pub fn run_init(shell: Option<HookShell>) -> Result<()> {
    let shell = match shell.or_else(HookShell::from_env) {
        Some(s) => s,
        None => bail!("could not detect shell; pass one explicitly: `berth shell init bash|zsh`"),
    };
    print!("{}", init_script(shell));
    Ok(())
}

pub fn run_completions(shell: Option<CompletionShell>) -> Result<()> {
    let shell = match shell {
        Some(s) => s,
        None => detect_completion_shell()
            .ok_or_else(|| anyhow::anyhow!(
                "could not detect shell; pass one explicitly: `berth shell completions bash|zsh|fish|elvish|powershell`"
            ))?,
    };
    let mut cmd = crate::cli::Cli::command();
    let mut buf: Vec<u8> = Vec::new();
    clap_complete::generate(shell, &mut cmd, "berth", &mut buf);
    let script = String::from_utf8(buf)
        .map_err(|e| anyhow::anyhow!("completion script was not valid utf-8: {e}"))?;

    let visible = hide_completion_artifacts(&script, shell);
    let augmented = match shell {
        CompletionShell::Zsh => augment_zsh(&visible),
        CompletionShell::Bash => augment_bash(&visible),
        _ => visible,
    };
    use io::Write;
    io::stdout().write_all(augmented.as_bytes())?;
    Ok(())
}

fn hide_completion_artifacts(script: &str, shell: CompletionShell) -> String {
    const HIDDEN_TOP: &[&str] = &[
        "list",
        "show",
        "new",
        "set",
        "rm",
        "daemon",
        "reap",
        "agent",
        "version-info",
        "hook-run",
    ];
    const HIDDEN_INTERNAL: &[&str] = &["daemon", "reap", "agent", "version-info", "hook-run"];
    const HIDDEN_ATTACH_FLAGS: &[&str] = &["resume-or-new", "supervisor", "session-counts"];
    const HIDDEN_ENTER_FLAGS: &[&str] = &["new"];

    match shell {
        CompletionShell::Bash => {
            let mut out = String::with_capacity(script.len());
            for line in script.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("opts=")
                    && line.contains("--resume-or-new")
                    && line.contains("--session-counts")
                {
                    out.push_str(&remove_long_flags(line, HIDDEN_ATTACH_FLAGS));
                    out.push('\n');
                } else if trimmed.starts_with("opts=") && line.contains("--new") {
                    out.push_str(&remove_long_flags(line, HIDDEN_ENTER_FLAGS));
                    out.push('\n');
                } else if line.trim_start().starts_with("opts=")
                    && line.contains("config list show new set rm enter")
                {
                    out.push_str("            opts=\"-v -q -h -V --verbose --quiet --help --version config enter attach stop run tunnel org hosts shell logs doctor deploy help\"\n");
                } else {
                    out.push_str(line);
                    out.push('\n');
                }
            }
            out
        }
        CompletionShell::Fish => {
            let mut out = String::with_capacity(script.len());
            for line in script.lines() {
                if fish_line_is_hidden(line, HIDDEN_TOP, HIDDEN_INTERNAL, HIDDEN_ATTACH_FLAGS) {
                    continue;
                }
                if line.contains("__fish_berth_using_subcommand enter") && line.contains("-l new") {
                    continue;
                }
                let line = sanitize_fish_seen_subcommand_list(line, HIDDEN_TOP);
                out.push_str(&line);
                out.push('\n');
            }
            out
        }
        CompletionShell::Zsh => {
            let mut out = String::with_capacity(script.len());
            let mut top_command_function = false;
            for line in script.lines() {
                let trimmed = line.trim();
                if line.contains("_berth_commands()")
                    || line.contains("_berth__subcmd__help_commands()")
                {
                    top_command_function = true;
                } else if top_command_function && trimmed == "}" {
                    top_command_function = false;
                }
                if zsh_line_is_hidden(
                    line,
                    HIDDEN_TOP,
                    HIDDEN_INTERNAL,
                    HIDDEN_ATTACH_FLAGS,
                    HIDDEN_ENTER_FLAGS,
                    top_command_function,
                ) {
                    continue;
                }
                out.push_str(line);
                out.push('\n');
            }
            out
        }
        CompletionShell::Elvish | CompletionShell::PowerShell => {
            hide_line_oriented_completion_artifacts(
                script,
                HIDDEN_TOP,
                HIDDEN_INTERNAL,
                HIDDEN_ATTACH_FLAGS,
                HIDDEN_ENTER_FLAGS,
            )
        }
        _ => hide_line_oriented_completion_artifacts(
            script,
            HIDDEN_TOP,
            HIDDEN_INTERNAL,
            HIDDEN_ATTACH_FLAGS,
            HIDDEN_ENTER_FLAGS,
        ),
    }
}

fn fish_line_is_hidden(
    line: &str,
    hidden_top: &[&str],
    hidden_internal: &[&str],
    hidden_flags: &[&str],
) -> bool {
    hidden_top.iter().any(|cmd| {
        line.contains("__fish_berth_needs_command") && line.contains(&format!("-a \"{cmd}\""))
            || line.contains("__fish_berth_using_subcommand help")
                && line.contains(&format!("-a \"{cmd}\""))
            || line.contains(&format!("__fish_berth_using_subcommand {cmd}\""))
    }) || hidden_internal
        .iter()
        .any(|cmd| line.contains(&format!("__fish_berth_using_subcommand {cmd}")))
        || hidden_flags.iter().any(|flag| {
            line.contains("__fish_berth_using_subcommand attach")
                && line.contains(&format!("-l {flag}"))
        })
}

fn sanitize_fish_seen_subcommand_list(line: &str, hidden_top: &[&str]) -> String {
    if !line.contains("__fish_seen_subcommand_from")
        || !line.contains("__fish_berth_using_subcommand help")
    {
        return line.to_string();
    }
    remove_words(line, hidden_top)
}

fn zsh_line_is_hidden(
    line: &str,
    hidden_top: &[&str],
    _hidden_internal: &[&str],
    hidden_flags: &[&str],
    hidden_enter_flags: &[&str],
    top_command_function: bool,
) -> bool {
    (top_command_function
        && hidden_top
            .iter()
            .any(|cmd| line.trim_start().starts_with(&format!("'{cmd}:"))))
        || line.contains("--interval-seconds")
        || line.contains("--once[Run one daemon")
        || hidden_flags
            .iter()
            .any(|flag| line.contains(&format!("--{flag}")))
        || hidden_enter_flags
            .iter()
            .any(|flag| line.contains(&format!("--{flag}[")))
}

fn hide_line_oriented_completion_artifacts(
    script: &str,
    hidden_top: &[&str],
    hidden_internal: &[&str],
    hidden_attach_flags: &[&str],
    hidden_enter_flags: &[&str],
) -> String {
    script
        .lines()
        .filter(|line| {
            !hidden_top.iter().any(|cmd| {
                line.contains(&format!(" {cmd} "))
                    || line.contains(&format!("'{cmd}'"))
                    || line.contains(&format!("\"{cmd}\""))
            }) && !hidden_internal.iter().any(|cmd| line.contains(cmd))
                && !hidden_attach_flags
                    .iter()
                    .any(|flag| line.contains(&format!("--{flag}")))
                && !hidden_enter_flags
                    .iter()
                    .any(|flag| line.contains(&format!("--{flag}")))
        })
        .map(|line| format!("{line}\n"))
        .collect()
}

fn remove_words(line: &str, words: &[&str]) -> String {
    let mut out = line.to_string();
    for word in words {
        out = out.replace(&format!(" {word}"), "");
        out = out.replace(&format!("{word} "), "");
    }
    out
}

fn remove_long_flags(line: &str, flags: &[&str]) -> String {
    let mut out = line.to_string();
    for flag in flags {
        out = out.replace(&format!(" --{flag}"), "");
    }
    out
}

pub fn run_hook_file(path: &Path) -> Result<()> {
    let line = std::fs::read_to_string(path)?;
    let argv = hook_argv(&line)?;
    let status = std::process::Command::new(std::env::current_exe()?)
        .env("BERTH_FROM_HOOK", "1")
        .env("BERTH_SKIP_AUTO", "1")
        .args(argv)
        .status()?;
    std::process::exit(status.code().unwrap_or(1));
}

fn hook_argv(line: &str) -> Result<Vec<String>> {
    let tokens = shell_words::split(line.trim())?;
    if tokens.len() < 4 {
        bail!("malformed hook invoke file");
    }
    let current_prefix = tokens[0] == "BERTH_FROM_HOOK=1"
        && tokens[1] == "BERTH_SKIP_AUTO=1"
        && tokens[2] == "command"
        && tokens[3] == "berth";
    let legacy_prefix = tokens[0] == "BERTH_SKIP_AUTO=1"
        && tokens[1] == "command"
        && tokens[2] == "berth"
        && tokens[3] == "enter";
    if current_prefix || legacy_prefix {
        let mut argv = if legacy_prefix {
            vec!["enter".to_string()]
        } else {
            Vec::new()
        };
        argv.extend(tokens[4..].iter().cloned());
        if argv.first().map(|s| s.as_str()) != Some("enter") {
            bail!("hook invoke file must run `berth enter`");
        }
        return Ok(argv);
    }
    bail!("malformed hook invoke file");
}

/// zsh: clap emits the workspace-name positional as `:_default`. Swap it
/// for our `_berth_workspaces` completer, do the same for `Org name` →
/// `_berth_orgs`, and prepend the helper function bodies.
fn augment_zsh(stock: &str) -> String {
    let helpers = ZSH_COMPLETER_HELPERS;
    let mut out = String::with_capacity(stock.len() + helpers.len() + 256);

    // Insert helpers immediately after the `#compdef berth` line so they're
    // defined before _berth references them.
    let mut lines = stock.lines();
    if let Some(first) = lines.next() {
        out.push_str(first);
        out.push('\n');
        out.push_str(helpers);
        out.push('\n');
    }
    for line in lines {
        let rewritten = rewrite_zsh_positional(line);
        out.push_str(&rewritten);
        out.push('\n');
    }
    out
}

fn rewrite_zsh_positional(line: &str) -> String {
    // Patterns clap generates, per `cli.rs` arg help text. Match on the
    // distinctive help-text prefix so we don't accidentally hit the Org
    // arg of `org set / show / rm` with workspace completion.
    if line.contains(":name -- Workspace name") && line.ends_with(":_default' \\") {
        return line.replace(":_default' \\", ":_berth_workspaces' \\");
    }
    if line.contains(":name -- Org name") && line.ends_with(":_default' \\") {
        return line.replace(":_default' \\", ":_berth_orgs' \\");
    }
    line.to_string()
}

const ZSH_COMPLETER_HELPERS: &str = r#"
# berth: dynamic completers — populate workspace/org names from the binary.
_berth_workspaces() {
    local -a names
    names=("${(@f)$(command berth list 2>/dev/null \
        | awk 'NR>1 && $1 != "" && $1 !~ /^-+$/ {print $1}')}")
    _describe -t workspaces 'workspace' names
}

_berth_orgs() {
    local -a names
    names=("${(@f)$(command berth org list 2>/dev/null \
        | awk '/^[[:space:]]+[A-Za-z0-9_./-]+:$/ {sub(/:$/, "", $1); print $1}')}")
    _describe -t orgs 'org' names
}
"#;

/// bash: clap's `_berth` dispatches to opts-only completion at positional
/// slots. Wrap it so that for known workspace/org positional positions we
/// inject the right candidate set; everything else falls through to clap.
fn augment_bash(stock: &str) -> String {
    let wrapper = BASH_COMPLETER_WRAPPER;
    format!("{stock}\n{wrapper}\n")
}

const BASH_COMPLETER_WRAPPER: &str = r#"
# berth: workspace-/org-name aware wrapper around the clap-generated `_berth`.
_berth_workspace_names() {
    command berth list 2>/dev/null \
        | awk 'NR>1 && $1 != "" && $1 !~ /^-+$/ {print $1}'
}

_berth_org_names() {
    command berth org list 2>/dev/null \
        | awk '/^[[:space:]]+[A-Za-z0-9_./-]+:$/ {sub(/:$/, "", $1); print $1}'
}

_berth_with_dynamic() {
    # Defer to clap first so flag completion and subcommand routing still work.
    _berth "$@"

    # Only override the candidate set when the user is on a positional slot
    # at the workspace/org-name position and the current word isn't a flag.
    local cur="${COMP_WORDS[COMP_CWORD]}"
    [[ "$cur" == -* ]] && return 0
    (( COMP_CWORD < 2 )) && return 0
    [[ -z "${COMP_WORDS[1]:-}" ]] && return 0

    local top="${COMP_WORDS[1]}"
    case "$top" in
        enter|stop|run|attach|tunnel)
            if (( COMP_CWORD == 2 )); then
                COMPREPLY=( $(compgen -W "$(_berth_workspace_names)" -- "$cur") )
            fi
            ;;
        org)
            # `berth org {show,set,rm} <name>` — org name at COMP_CWORD == 3.
            if (( COMP_CWORD == 3 )) && [[ "${COMP_WORDS[2]:-}" =~ ^(show|set|rm)$ ]]; then
                COMPREPLY=( $(compgen -W "$(_berth_org_names)" -- "$cur") )
            fi
            ;;
    esac
}
complete -F _berth_with_dynamic -o nosort -o bashdefault -o default berth
"#;

fn detect_completion_shell() -> Option<CompletionShell> {
    let shell = std::env::var("SHELL").ok()?;
    let base = std::path::Path::new(&shell)
        .file_name()
        .and_then(|s| s.to_str())?;
    match base {
        "bash" => Some(CompletionShell::Bash),
        "zsh" => Some(CompletionShell::Zsh),
        "fish" => Some(CompletionShell::Fish),
        "elvish" => Some(CompletionShell::Elvish),
        "pwsh" | "powershell" => Some(CompletionShell::PowerShell),
        _ => None,
    }
}

fn init_script(shell: HookShell) -> String {
    let common = COMMON_PRELUDE;
    let body = match shell {
        HookShell::Bash => BASH_HOOK,
        HookShell::Zsh => ZSH_HOOK,
    };
    format!("{common}\n{body}\n{COMMON_EPILOGUE}\n")
}

const COMMON_PRELUDE: &str = r#"# berth shell integration: new-tab auto-entry hook
# Generated by `berth shell init`. Re-run after upgrading berth.
#
# Auto-entry signal:
#   OSC 7 inherited PWD marker under ~/.local/state/berth/active.
#
# When the marker path is detected, the hook `cd`s to $HOME *before*
# invoking `berth enter`, so the user is never parked in the marker dir.
#
# OPT OUT (one shot):   BERTH_SKIP_AUTO=1 <command>
# OPT OUT (this shell): export BERTH_SKIP_AUTO=1

_berth_state_dir() {
    printf '%s/berth/active' "${XDG_STATE_HOME:-$HOME/.local/state}"
}

_berth_detect_project() {
    if [ -n "${BERTH_PROJECT_HINT:-}" ]; then
        printf '%s' "$BERTH_PROJECT_HINT"
        return 0
    fi
    local state_dir dir_name canonical
    state_dir="$(_berth_state_dir)"
    case "$PWD/" in
        "$state_dir"/*/*)
            local rest="${PWD#$state_dir/}"
            dir_name="${rest%%/*}"
            ;;
        "$state_dir"/*)
            dir_name="${PWD#$state_dir/}"
            ;;
        *)
            return 1
            ;;
    esac
    # Prefer the canonical name written by `berth enter` (preserves
    # slashes); fall back to the directory basename only if the marker
    # file is missing (legacy / external state).
    if [ -r "$state_dir/$dir_name/.workspace" ]; then
        canonical="$(cat "$state_dir/$dir_name/.workspace" 2>/dev/null)"
        [ -n "$canonical" ] && { printf '%s' "$canonical"; return 0; }
    fi
    printf '%s' "$dir_name"
    return 0
}

_berth_auto_enter_on_start() {
    [ -n "${BERTH_SKIP_AUTO:-}" ] && return 0
    [ -n "${BERTH_WORKSPACE:-}" ] && return 0
    case "$-" in
        *i*) : ;;
        *) return 0 ;;
    esac

    local state_dir invoke_file invoke_line proj status
    state_dir="$(_berth_state_dir)"

    # 1. Cwd-inheritance path: detect workspace from $PWD if it's inside
    #    the marker dir tree. Hop out to $HOME before doing anything
    #    visible so the user is never parked in the state dir.
    proj="$(_berth_detect_project 2>/dev/null)" || proj=""
    if [ -n "$proj" ]; then
        case "$PWD/" in
            "$state_dir"/*) cd "$HOME" 2>/dev/null || cd / ;;
        esac
        invoke_file="$state_dir/$(printf '%s' "$proj" | sed 's|/|_|g')/.invoke"
        if [ -r "$invoke_file" ]; then
            # Run the exact `berth enter <ws> [--dir D] [-- argv]`
            # line written by the parent tab. Replays workspace + dir +
            # command override verbatim. Parsing is delegated back to
            # berth so this hook does not eval file contents as shell.
            invoke_line="$(cat "$invoke_file" 2>/dev/null)"
            case "$invoke_line" in
                "BERTH_FROM_HOOK=1 BERTH_SKIP_AUTO=1 command berth enter "*)
                    command berth hook-run "$invoke_file"
                    status=$?
                    if [ "$status" -ne 0 ]; then
                        printf 'berth: auto-enter failed. Skipping.\n' >&2
                    fi
                    return "$status"
                    ;;
                "BERTH_SKIP_AUTO=1 command berth enter --new "*)
                    command berth hook-run "$invoke_file"
                    status=$?
                    if [ "$status" -ne 0 ]; then
                        printf 'berth: auto-enter failed. Skipping.\n' >&2
                    fi
                    return "$status"
                    ;;
                ?*)
                    printf 'berth: ignoring malformed .invoke (%s)\n' "$invoke_file" >&2
                    ;;
            esac
        fi
        # Fall back to bare invocation if marker exists but no .invoke.
        BERTH_FROM_HOOK=1 BERTH_SKIP_AUTO=1 command berth enter "$proj"
        return $?
    fi
    return 0
}

# Defensive: if $PWD has vanished out from under us (older berth versions
# created and then deleted a state dir while a shell was cwd'd inside it),
# silently cd $HOME so direnv, getwd(), and `cd ..` stop failing on every
# prompt. Cheap to keep; harmless when $PWD is fine.
_berth_cwd_heal() {
    [ -d "$PWD" ] && return 0
    cd "$HOME" 2>/dev/null || cd /
}

"#;

const BASH_HOOK: &str = r#"# bash: run detection once per interactive shell start, and self-heal
# the cwd before every prompt.
if [ -n "${BASH_VERSION:-}" ]; then
    case ";${PROMPT_COMMAND:-};" in
        *";_berth_cwd_heal;"*) : ;;
        *) PROMPT_COMMAND="_berth_cwd_heal;${PROMPT_COMMAND:+$PROMPT_COMMAND}" ;;
    esac
    _berth_auto_enter_on_start
fi
"#;

const ZSH_HOOK: &str = r#"# zsh: same — run once at startup, plus precmd self-heal.
if [ -n "${ZSH_VERSION:-}" ]; then
    autoload -Uz add-zsh-hook
    add-zsh-hook precmd _berth_cwd_heal
    _berth_auto_enter_on_start
fi
"#;

const COMMON_EPILOGUE: &str = r#"# end berth shell integration
"#;

#[cfg(test)]
mod tests {
    use super::hook_argv;

    #[test]
    fn hook_argv_accepts_current_format() {
        let argv = hook_argv(
            "BERTH_FROM_HOOK=1 BERTH_SKIP_AUTO=1 command berth enter 'org/proj' -- 'echo' 'a && b'",
        )
        .unwrap();
        assert_eq!(argv, ["enter", "org/proj", "--", "echo", "a && b"]);
    }

    #[test]
    fn hook_argv_accepts_legacy_format() {
        let argv = hook_argv("BERTH_SKIP_AUTO=1 command berth enter --new 'org/proj'").unwrap();
        assert_eq!(argv, ["enter", "--new", "org/proj"]);
    }

    #[test]
    fn hook_argv_rejects_non_enter() {
        assert!(hook_argv("BERTH_FROM_HOOK=1 BERTH_SKIP_AUTO=1 command berth logs").is_err());
    }
}
