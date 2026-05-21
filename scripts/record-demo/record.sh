#!/usr/bin/env bash
# Re-record the README's demo asset (assets/demo.svg).
#
# Builds the sandbox + pre-captures doctor output FIRST, outside
# asciinema. Then runs asciinema against the (much shorter) demo.sh
# driver, so the captured cast starts at the first visible byte
# instead of with seconds of silent setup. Finally scrubs the tmpdir
# prefix out of the cast and renders to SVG via svg-term-cli.
#
# Requirements on the host: cargo, asciinema, svg-term-cli (`npm i -g
# svg-term-cli` or via volta), and podman/kubectl/minikube on PATH if
# you want the recorded `berth doctor` rows to show as ready.

set -euo pipefail

repo_root="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
cd "$repo_root"

for tool in asciinema svg-term cargo; do
    command -v "$tool" >/dev/null || {
        echo "missing tool on PATH: $tool" >&2
        exit 1
    }
done

cargo build --release --quiet

# ── Sandbox prep (silent; nothing captured yet) ─────────────────────
# We DO NOT export HOME or PATH here. asciinema is a Python script
# whose `import` discovery relies on $HOME pointing at the recorder's
# real homedir; clobbering it makes the next `asciinema rec` fail
# with ModuleNotFoundError. So instead we hold sandbox env in a
# bash array and apply it as a one-shot `env -S` prefix only where
# berth actually needs it (the setup `berth …` calls below, and the
# recorded child shell).
src_berth="$repo_root/target/release/berth"
demo_dir="$(mktemp -d -p "${TMPDIR:-/tmp}" berth-demo.XXXXXX)"
sandbox_home="$demo_dir/home/dev"
sandbox_bin="$demo_dir/home/dev/.local/bin"
mkdir -p "$sandbox_home/code/app" "$sandbox_home/code/docs" "$sandbox_bin"
install -m 0755 "$src_berth" "$sandbox_bin/berth"

# Plant the hook line in the sandbox rc so doctor reports
# "Hook installed: yes" without a long install hint.
printf '%s\n' 'eval "$(berth shell init)"' > "$sandbox_home/.bashrc"

mkdir -p "$sandbox_home/.config" \
         "$sandbox_home/.local/share/berth/projects" \
         "$sandbox_home/.local/state"

# Env applied per-invocation. CLICOLOR_FORCE makes the `colored`
# crate emit ANSI even when stdout is captured via redirection
# (otherwise the cached doctor output would be monochrome).
sandbox_env=(
    "HOME=$sandbox_home"
    "PATH=$sandbox_bin:$PATH"
    "XDG_CONFIG_HOME=$sandbox_home/.config"
    "XDG_DATA_HOME=$sandbox_home/.local/share"
    "XDG_STATE_HOME=$sandbox_home/.local/state"
    "SHELL=/bin/bash"
    "CLICOLOR_FORCE=1"
)

# Seed workspaces silently.
env "${sandbox_env[@]}" berth new app "$sandbox_home/code/app" \
    >/dev/null 2>&1
env "${sandbox_env[@]}" berth new docs "$sandbox_home/code/docs" \
    >/dev/null 2>&1
env "${sandbox_env[@]}" berth set app --dir '~/code/app' \
    -- bash -c 'cd ~/code/app && assist' >/dev/null 2>&1

# Pre-capture doctor's output. Real `berth doctor` takes ~1.5s
# because of parallel runtime probes; in a recording that just looks
# like the CLI is slow. We cache the bytes and replay them via `cat`
# inside the driver — same content, instant playback.
doctor_cache="$demo_dir/doctor.out"
env "${sandbox_env[@]}" berth doctor > "$doctor_cache" 2>&1

# ── Record (everything below this point is captured) ────────────────
# asciinema runs with the *real* HOME/PATH so its Python imports work.
# It spawns a child shell that picks up the sandbox env via `env …`,
# so the recorded `berth` invocations see the sandbox.
mkdir -p .cache/demo-record
cast=.cache/demo-record/demo.cast
scrubbed=.cache/demo-record/demo.cast.scrubbed

env_prefix=()
for kv in "${sandbox_env[@]}"; do env_prefix+=("$kv"); done
env_prefix+=("BERTH_DOCTOR_CACHE=$doctor_cache")
env_prefix+=("BERTH_DEMO_SANDBOX_PREFIX=$demo_dir")

# Shell-quote the env vars so the asciinema -c argument is one
# coherent shell command.
quoted_env=""
for kv in "${env_prefix[@]}"; do
    quoted_env+=" $(printf '%q' "$kv")"
done

asciinema rec \
    -c "env$quoted_env bash $repo_root/scripts/record-demo/demo.sh" \
    --rows 24 --cols 100 \
    --quiet --overwrite "$cast"

# Scrub the sandbox tmpdir prefix out of the cast so the rendered
# SVG shows clean `/home/dev/...` paths.
prefix="$(grep -oE '/tmp/berth-demo\.[A-Za-z0-9]+' "$cast" | head -n1 || true)"
if [[ -n "$prefix" ]]; then
    sed "s|$prefix/home/dev|/home/dev|g; s|$prefix||g" "$cast" > "$scrubbed"
else
    cp "$cast" "$scrubbed"
fi

svg-term \
    --in "$scrubbed" \
    --out assets/demo.svg \
    --width 100 --height 24 \
    --window

rm -rf "$demo_dir"

# Sanity-check that no real identifier slipped through.
if grep -qE '/home/me|morgaesis|kristofer' assets/demo.svg; then
    echo "leaked identifier in assets/demo.svg — aborting" >&2
    grep -oE '/home/me|morgaesis|kristofer' assets/demo.svg | sort -u
    exit 1
fi

echo "wrote assets/demo.svg ($(stat -c %s assets/demo.svg) bytes)"
