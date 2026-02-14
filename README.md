# Berth

Consistent development workspaces, local or remote, bare metal.

## Features

- **Bare metal by default** - No containers, just your shell
- **Remote or local** - Same workflow, different targets
- **Project isolation** - Each workspace is independent
- **Port forwarding** - Remote ports appear as `localhost:<port>`
- **Hostname resolution** - `http://myproject.berth` works for any project
- **Auto-spawn** - Ctrl+Shift+T opens new terminal in current project

## Usage

```bash
# Create a workspace
berth new myproject ~/projects/myproject

# Enter a workspace (local)
berth enter myproject

# Enter a workspace (remote)
berth enter myproject --remote user@host

# List workspaces
berth list

# Tunnel remote ports locally
berth tunnel myproject --ports 3000,8080

# Stop a workspace
berth stop myproject
```

## Configuration

Config file at `~/.config/berth/config.yaml` or `~/.config/berth/config.json`:

```yaml
workspaces:
  myproject:
    path: ~/projects/myproject
    remote: user@host  # optional
    ports: [3000, 8080]  # optional, for tunneling
```

## Hostname Resolution

Berth adds entries to `/etc/hosts` for each project:

```
127.0.0.1 myproject.berth
127.0.0.1 another.berth
```

Combined with port forwarding, `http://myproject.berth:3000` works seamlessly.

## Shell Integration

Add to your `~/.bashrc` or `~/.zshrc`:

```bash
eval "$(berth init-shell)"
```

This enables:
- Terminal title shows current workspace
- Ctrl+Shift+T spawns new terminal in same workspace
- `berth` auto-enters if cwd is in a workspace
