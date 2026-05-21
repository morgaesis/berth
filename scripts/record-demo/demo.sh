#!/usr/bin/env bash
# Demo driver — paced sequence of berth commands captured by
# asciinema. Each command line is "typed" character-by-character so
# the rendered SVG plays back as if a human is at the keyboard.
#
# Invoked as:
#   asciinema rec -c "bash scripts/record-demo/demo.sh" demo.cast
#
# Everything visible in the recording (paths, $HOME, the resolved
# berth binary location) is rerooted under a tmpdir so the captured
# frame never includes the recorder's real username or filesystem
# layout. The outer record.sh wrapper sed-scrubs the tmpdir prefix
# back to /home/dev before rendering.

set -euo pipefail

# Resolve the release binary on the host.
repo_root="$(git rev-parse --show-toplevel)"
src_berth="$repo_root/target/release/berth"
if [[ ! -x "$src_berth" ]]; then
    echo "build target/release/berth first" >&2
    exit 1
fi

# Build a sandbox that pretends to be a generic developer home. The
# tmpdir prefix lives in $BERTH_DEMO_SANDBOX_PREFIX so the outer
# recording wrapper can sed-replace it back to `/home/dev` in the
# captured cast — keeps the rendered frame free of `/tmp/…` noise.
demo_dir="$(mktemp -d -p "${TMPDIR:-/tmp}" berth-demo.XXXXXX)"
export BERTH_DEMO_SANDBOX_PREFIX="$demo_dir"
sandbox_home="$demo_dir/home/dev"
sandbox_bin="$demo_dir/home/dev/.local/bin"
mkdir -p "$sandbox_home/code/app" "$sandbox_home/code/docs" "$sandbox_bin"
install -m 0755 "$src_berth" "$sandbox_bin/berth"

# Reroot everything berth reads. PATH is rewritten so the sandbox
# berth wins; $HOME and the XDG vars send config/state under the
# sandbox so the visible paths in the recording stay generic.
export HOME="$sandbox_home"
export PATH="$sandbox_bin:/usr/bin:/bin"
export XDG_CONFIG_HOME="$sandbox_home/.config"
export XDG_DATA_HOME="$sandbox_home/.local/share"
export XDG_STATE_HOME="$sandbox_home/.local/state"
export CLICOLOR_FORCE=1
# Force a generic SHELL so `berth doctor` reports a stable value and
# reads from a single rc file we control, not whatever the recorder's
# real $SHELL happens to be.
export SHELL=/bin/bash
mkdir -p "$XDG_CONFIG_HOME" "$XDG_DATA_HOME/berth/projects" "$XDG_STATE_HOME"

# Pre-install the new-tab hook in the sandbox rc file so doctor reports
# "Hook installed: yes" instead of the long install hint. doctor greps
# for "berth shell init" / "berth shell-init"; the line just has to
# contain the literal.
printf '%s\n' 'eval "$(berth shell init)"' > "$HOME/.bashrc"

cd "$HOME"

# Seed workspaces BEFORE the recording starts so the demo plays as
# one continuous session — no silent gap between an empty list and a
# populated list, no mid-recording `clear`. The viewer sees doctor,
# then list with stuff already in it, then show, then the enter
# explanation, then list again as a closing shot.
berth new app ~/code/app >/dev/null 2>&1
berth new docs ~/code/docs >/dev/null 2>&1
berth set app --dir '~/code/app' -- bash -c 'cd ~/code/app && assist' >/dev/null 2>&1

# Pre-capture the actual doctor output once, before recording starts.
# Real `berth doctor` takes ~1.5s (it subprocesses out to podman /
# minikube to check their health, in parallel). Faithful to the CLI
# on a host with those tools — but in a *recorded* demo it just looks
# like berth itself is slow. We replay the cached bytes during the
# recording so the rendered output stays instant.
doctor_output="$(berth doctor 2>&1)"

# Colors for the demo script's own chrome (the prompt + captions).
# Berth's own output keeps its own coloring; nothing here re-colors it.
green='\033[1;32m'
dim='\033[2m'
reset='\033[0m'

# Per-keystroke delays for the typing animation. Picked at random from
# this list to mimic the natural cadence of someone actually typing —
# flat 40ms looks robotic. Berth itself runs at native speed (no fake
# pauses); the only delay around a command is the 300ms gap right
# after the `$` prompt shows, when a person reads what they typed
# last and reaches for the keyboard.
type_delays=('0.025' '0.035' '0.045' '0.05' '0.055' '0.07' '0.04')
# Between-command "reading" pause. The viewer needs a beat to scan the
# previous output before the next command starts typing — 2–3s with a
# bit of jitter feels close to natural reading.
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

# Print the `$` prompt with a brief cursor-blink pause, then type the
# command. Newline goes out immediately so the real `berth` invocation
# below runs at native speed — no artificial delay between Enter and
# the first byte of output.
say() {
    printf '%b$%b ' "$green" "$reset"
    sleep "$(random_from "${prompt_delays[@]}")"
    type_out "$1"
}

caption() {
    printf '%b%s%b\n' "$dim" "$*" "$reset"
}

clear

say "berth doctor"
printf '%s\n' "$doctor_output"

say "berth list"
berth list

say "berth show app"
berth show app

# `berth enter` would drop us into an interactive shell, which would
# block the recording. Show the bundled argv with a dim caption
# explaining the new-tab behaviour instead.
say "berth enter app"
caption '  # opens an interactive session running the bundled argv:'
caption '  #     bash -c "cd ~/code/app && assist"'
caption '  # meta+T (or Ctrl+Shift+T) opens another tab with the'
caption '  # same setup — independent session, same environment.'

# Four dim lines of explanation deserve real reading time before the
# next command starts typing. The next `say`'s own prompt-pause adds
# another ~2.5s on top; total caption→next-command gap is ~7s.
sleep 4.5

# Close on `berth list` — same continuous session, no clear, no
# silent gap. The viewer sees the seeded workspaces one last time,
# then a trailing $ cursor.
say "berth list"
berth list

printf '%b$%b ' "$green" "$reset"
sleep 2.5

rm -rf "$demo_dir"
