# Installing berth

This page is written so a coding agent can follow it without other context.
Humans get the same instructions; the agent path is the human path, just
written out.

After installing, verify with `berth --version` — the command must succeed
and print a semver string. If it doesn't, none of the methods below
finished correctly; do not declare success.

## TL;DR

| You have | Run |
| --- | --- |
| Rust toolchain | `cargo install --git https://github.com/morgaesis/berth --locked` |
| `tooler` | `tooler run morgaesis/berth` |
| `gh` + a tagged release on GitHub | `gh release download --repo morgaesis/berth --pattern 'berth-*-linux-x86_64*' && install -m755 berth-*-linux-x86_64 ~/.local/bin/berth` |
| Only `podman` | See [Try without installing](#try-without-installing) |

`~/.local/bin` must be on `PATH`. On most systems this is already true; if
not, add `export PATH="$HOME/.local/bin:$PATH"` to your shell rc.

## Pick a method

### 1. From source (`cargo install`)

Requires a Rust toolchain (≥ 1.75) and a C linker (`build-essential` on
Debian/Ubuntu, `base-devel` on Arch, Xcode CLT on macOS).

```bash
cargo install --git https://github.com/morgaesis/berth --locked
```

`cargo install` writes the binary to `~/.cargo/bin/berth`. Either ensure
`~/.cargo/bin` is on `PATH` or symlink it into `~/.local/bin`:

```bash
ln -sf ~/.cargo/bin/berth ~/.local/bin/berth
```

### 2. Prebuilt binary from GitHub Releases

Releases ship musl-static Linux binaries for x86_64 and aarch64, plus macOS
universal builds. Pick the asset matching your platform.

```bash
# Example for Linux x86_64 — adjust for your platform.
gh release download \
  --repo morgaesis/berth \
  --pattern 'berth-*-linux-x86_64*' \
  --output - | install /dev/stdin ~/.local/bin/berth -m 0755
```

Without `gh`, use `curl` against the asset URL from the releases page.
Verify the SHA256 against the `.sha256` companion file before installing.

### 3. From a local clone (development)

```bash
git clone https://github.com/morgaesis/berth.git
cd berth
cargo build --release --quiet
install -m755 target/release/berth ~/.local/bin/berth
```

Or symlink the build output so subsequent `cargo build --release` swaps
the binary in place:

```bash
ln -sf "$PWD/target/release/berth" ~/.local/bin/berth
```

### 4. With `tooler`

```bash
tooler run morgaesis/berth
```

`tooler` resolves the latest release for the platform it's running on and
drops the binary on `PATH`. Use this if you already manage tools through
tooler — there is no berth-specific configuration to do.

## Try without installing

```bash
podman run --rm -it -v "$PWD":/work -w /work \
  ghcr.io/morgaesis/berth:latest \
  berth doctor
```

The container is a self-contained way to kick the tires (the published
image bundles podman + kubectl + minikube so `berth doctor` shows
something interesting). It is not a recommended long-term install path —
remote entry, deploy, and the shell hook all assume berth runs on the
host.

## Shell completion and the new-tab hook

Completions complete subcommand names, flags, and **workspace names** from
`berth list`. Install in your rc file:

```bash
# zsh — pick one
eval "$(berth shell completions)"                 # source on every shell start
berth shell completions zsh > ~/.zsh/completions/_berth   # cache to fpath

# bash
eval "$(berth shell completions)"
# or
berth shell completions bash > ~/.local/share/bash-completion/completions/berth
```

The new-tab auto-entry hook is opt-in. It makes new terminal tabs spawned
from inside a berth session re-enter the same workspace + command.

```bash
eval "$(berth shell init)"
```

`berth doctor` reports whether the hook is present in your rc files.

## Verify

```bash
berth --version           # must print a semver line
berth doctor              # local runtime + shell-hook status
berth list                # empty table on a fresh install — that's expected
```

If you intend to use berth on remote hosts, also run:

```bash
berth deploy <ssh-host>   # one-shot deploy of the same version to the remote
```

On first `berth enter --remote <host> <workspace>`, berth offers to do
this automatically and remembers consent in config.

## Uninstall

```bash
rm -f ~/.local/bin/berth ~/.cargo/bin/berth
rm -rf ~/.config/berth ~/.local/share/berth ~/.local/state/berth
```

Remove any `eval "$(berth shell …)"` lines from your shell rc as well.
