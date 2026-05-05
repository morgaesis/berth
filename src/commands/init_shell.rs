use anyhow::Result;

pub fn run() -> Result<()> {
    print!("{}", script());
    Ok(())
}

fn script() -> &'static str {
    r#"# berth shell integration
# Usage: eval "$(berth init-shell)"
# Supported shells: bash, zsh

if [ -z "${BASH_VERSION:-}" ] && [ -z "${ZSH_VERSION:-}" ]; then
    printf 'berth: init-shell only supports bash and zsh\n' >&2
    return 0 2>/dev/null || true
else

_berth_enter_name() {
    if [ "$#" -eq 0 ]; then
        command berth enter
        return $?
    fi
    command berth enter "$@"
    local exit_code=$?
    if [ "$exit_code" -eq 0 ]; then
        export BERTH_SKIP_AUTO=1
    fi
    return $exit_code
}

b() {
    _berth_enter_name "$@"
}

_berth_auto_enter() {
    [ -n "${BERTH_SKIP_AUTO:-}" ] && return 0
    local berth_config="${XDG_CONFIG_HOME:-$HOME/.config}/berth"
    local config_file=""
    if [ -f "$berth_config/config.yaml" ]; then
        config_file="$berth_config/config.yaml"
    elif [ -f "$berth_config/config.json" ]; then
        config_file="$berth_config/config.json"
    else
        return 0
    fi

    local cwd="$PWD"
    while [ "$cwd" != "/" ] && [ -n "$cwd" ]; do
        if grep -q -F "$cwd" "$config_file" 2>/dev/null; then
            local ws_name
            ws_name="$(basename "$cwd")"
            _berth_enter_name "$ws_name"
            return $?
        fi
        cwd="$(dirname "$cwd")"
    done
}

_berth_set_title() {
    if [ -n "${BERTH_WORKSPACE:-}" ]; then
        printf '\033]2;berth: %s\033\\\033]1;berth: %s\033\\' "$BERTH_WORKSPACE" "$BERTH_WORKSPACE"
    fi
}

_berth_set_prompt() {
    if [ -n "${BERTH_WORKSPACE:-}" ] && [ -z "${_BERTH_PROMPT_PATCHED:-}" ]; then
        PS1="[berth:$BERTH_WORKSPACE] $PS1"
        export _BERTH_PROMPT_PATCHED=1
    fi
}

berth() {
    case "${1:-}" in
        enter)
            shift
            _berth_enter_name "$@"
            return $?
            ;;
        *)
            command berth "$@"
            ;;
    esac
}

if [ -n "${ZSH_VERSION:-}" ]; then
    autoload -Uz add-zsh-hook
    add-zsh-hook chpwd _berth_auto_enter
    add-zsh-hook precmd _berth_set_title
    add-zsh-hook precmd _berth_set_prompt
elif [ -n "${BASH_VERSION:-}" ]; then
    if ! declare -f _berth_orig_cd >/dev/null 2>&1; then
        eval "_berth_orig_cd() { builtin cd \"\$@\"; }"
        cd() {
            _berth_orig_cd "$@" || return $?
            _berth_auto_enter
        }
    fi
    case ";${PROMPT_COMMAND:-};" in
        *";_berth_set_title;_berth_set_prompt;"*) : ;;
        *) PROMPT_COMMAND="_berth_set_title;_berth_set_prompt;${PROMPT_COMMAND:+$PROMPT_COMMAND}" ;;
    esac
fi

[ -n "${BERTH_WORKSPACE:-}" ] && _berth_set_title
_berth_auto_enter

fi
"#
}
