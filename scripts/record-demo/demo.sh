#!/usr/bin/env bash
# Demo driver — paced sequence of berth commands captured by
# asciinema. Each command line is "typed" character-by-character so
# the rendered SVG plays back as if a human is at the keyboard.
#
# All sandbox setup (creating /tmp/berth-demo.XXXX, installing the
# binary, planting the rc-file hook, seeding workspaces, pre-capturing
# doctor output) happens in record.sh BEFORE asciinema starts. This
# script assumes the env is ready and runs only the visible commands —
# that way the cast doesn't start with seconds of silent setup.
#
# Required env (set by record.sh):
#   BERTH_DOCTOR_CACHE   — path to file containing pre-captured doctor output
#   HOME, PATH, XDG_*    — already pointed at the sandbox
#
# Optional:
#   BERTH_DEMO_TYPE_DELAYS, BERTH_DEMO_PROMPT_DELAYS — cadence overrides

set -euo pipefail

: "${BERTH_DOCTOR_CACHE:?record.sh must set BERTH_DOCTOR_CACHE}"

# Colors for the demo script's own chrome (the prompt + captions).
green='\033[1;32m'
dim='\033[2m'
reset='\033[0m'

# Per-keystroke delays. Picked at random from this list per char to
# mimic the cadence of someone typing.
type_delays=('0.025' '0.035' '0.045' '0.05' '0.055' '0.07' '0.04')
# Between-command "reading" pause — viewer needs a beat to scan the
# previous output before the next command types in. 2-3s with jitter.
prompt_delays=('2.1' '2.4' '2.8' '2.2' '3.0' '2.5')

random_from() {
    local arr=("$@")
    printf '%s' "${arr[$((RANDOM % ${#arr[@]}))]}"
}

type_out() {
    local s="$1" i
    for ((i=0; i<${#s}; i++)); do
        printf '%s' "${s:$i:1}"
        sleep "$(random_from "${type_delays[@]}")"
    done
    printf '\n'
}

say() {
    printf '%b$%b ' "$green" "$reset"
    sleep "$(random_from "${prompt_delays[@]}")"
    type_out "$1"
}

caption() {
    printf '%b%s%b\n' "$dim" "$*" "$reset"
}

# Scene 1: doctor — fills most of the viewport on its own.
say "berth doctor"
cat "$BERTH_DOCTOR_CACHE"

# Brief pause so the viewer can read, then clear before scene 2. This
# is the only clear in the recording — it exists because doctor's
# output is ~11 lines and the rest of the demo is another ~15;
# together they'd scroll off a 24-row viewport.
sleep 3.5
clear

# Scene 2: the workspace flow — list, show, the enter explanation,
# then list again as a closing beat.
say "berth list"
berth list

say "berth show app"
berth show app

# `berth enter` would drop into an interactive session and block the
# recording. Print the bundled argv with a dim caption explaining the
# new-tab behaviour instead.
say "berth enter app"
caption '  # opens an interactive session running the bundled argv:'
caption '  #     bash -c "cd ~/code/app && assist"'
caption '  # meta+T (or Ctrl+Shift+T) opens another tab with the'
caption '  # same setup — independent session, same environment.'

# Four dim lines of explanation need real reading time. The next
# say's prompt-pause adds another ~2.5s; total ~7s before the next
# command starts typing.
sleep 4.5

say "berth list"
berth list

# Trailing cursor so the recording ends on a stable state.
printf '%b$%b ' "$green" "$reset"
sleep 1.2
