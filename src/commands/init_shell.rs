use anyhow::Result;

pub fn run() -> Result<()> {
    let _shell = std::env::var("SHELL").unwrap_or_default();

    let script = r#"
_berth_auto_enter() {
    local cwd="${PWD}"
    local berth_config="$HOME/.config/berth"
    local config_file
    
    if [[ -f "$berth_config/config.yaml" ]]; then
        config_file="$berth_config/config.yaml"
    elif [[ -f "$berth_config/config.json" ]]; then
        config_file="$berth_config/config.json"
    else
        return
    fi
    
    while [[ "$cwd" != "/" ]]; do
        if grep -q "$cwd" "$config_file" 2>/dev/null; then
            local ws_name=$(basename "$cwd")
            if [[ -n "$BERTH_SKIP_AUTO" ]]; then
                return
            fi
            export BERTH_SKIP_AUTO=1
            berth enter "$ws_name"
            return
        fi
        cwd=$(dirname "$cwd")
    done
}

_berth_chpwd() {
    _berth_auto_enter
}

_berth_set_title() {
    if [[ -n "$BERTH_WORKSPACE" ]]; then
        printf '\033]2;berth: %s\033\\\033]1;berth: %s\033\\' "$BERTH_WORKSPACE" "$BERTH_WORKSPACE"
    fi
}

_berth_set_prompt() {
    if [[ -n "$BERTH_WORKSPACE" ]]; then
        PS1="[berth] $PS1"
    fi
}

berth() {
    case "$1" in
        enter)
            command berth "$@"
            local exit_code=$?
            if [[ $exit_code -eq 0 ]]; then
                export BERTH_SKIP_AUTO=1
            fi
            return $exit_code
            ;;
        *)
            command berth "$@"
            ;;
    esac
}

if [[ -n "$BERTH_WORKSPACE" ]]; then
    _berth_set_title
fi

if [[ -n "$ZSH_VERSION" ]]; then
    autoload -U add-zsh-hook
    add-zsh-hook chpwd _berth_chpwd
    add-zsh-hook precmd _berth_set_title
    add-zsh-hook precmd _berth_set_prompt
elif [[ -n "$BASH_VERSION" ]]; then
    cd() {
        builtin cd "$@"
        _berth_chpwd
    }
    PROMPT_COMMAND="_berth_set_title;_berth_set_prompt;${PROMPT_COMMAND:+$PROMPT_COMMAND;}"
fi

_berth_auto_enter
"#;

    println!("{}", script);
    Ok(())
}
