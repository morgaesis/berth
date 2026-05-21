#!/usr/bin/env bash
# Re-record the README's demo asset (assets/demo.svg).
#
# Runs asciinema against scripts/record-demo/demo.sh, scrubs the
# captured cast so the tmpdir prefix vanishes, then renders the cast
# to SVG with svg-term-cli.
#
# Requirements on the host: cargo, asciinema, svg-term-cli (`npm i -g
# svg-term-cli` or via volta), and podman/kubectl/minikube on PATH if
# you want the recorded `berth doctor` rows to show as ready.

set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

for tool in asciinema svg-term cargo; do
    command -v "$tool" >/dev/null || {
        echo "missing tool on PATH: $tool" >&2
        exit 1
    }
done

cargo build --release --quiet

mkdir -p .cache/demo-record
cast=.cache/demo-record/demo.cast
scrubbed=.cache/demo-record/demo.cast.scrubbed

asciinema rec \
    -c "bash scripts/record-demo/demo.sh" \
    --rows 24 --cols 100 \
    --quiet --overwrite "$cast"

# Scrub the sandbox tmpdir prefix out of the cast so the rendered SVG
# shows a clean `/home/dev/...` path instead of `/tmp/berth-demo.XXXXXX/...`.
# The driver script exports BERTH_DEMO_SANDBOX_PREFIX before running
# berth; we read it from the cast itself via a marker line.
prefix="$(jq -r '
    select(.[1] == "o") | .[2]
    ' "$cast" 2>/dev/null \
    | grep -oE '/tmp/berth-demo\.[A-Za-z0-9]+' \
    | head -n1 || true)"
if [[ -z "$prefix" ]]; then
    # asciinema 2.x writes a header line then one event per line.
    # Newer asciinema (cast v3) uses a different format; fall back to
    # plain grep on the raw cast bytes.
    prefix="$(grep -oE '/tmp/berth-demo\.[A-Za-z0-9]+' "$cast" | head -n1 || true)"
fi
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

# Sanity-check that no real identifier slipped through. Add patterns
# here as they come up.
if grep -qE '/home/me|morgaesis|kristofer' assets/demo.svg; then
    echo "leaked identifier in assets/demo.svg — aborting" >&2
    grep -oE '/home/me|morgaesis|kristofer' assets/demo.svg | sort -u
    exit 1
fi

echo "wrote assets/demo.svg ($(stat -c %s assets/demo.svg) bytes)"
