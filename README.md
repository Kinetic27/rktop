<div align="center">
  <img src="assets/logo.svg" alt="rktop" width="520">

  <p><strong>A btop-inspired TUI dashboard for monitoring multiple Linux servers over SSH.</strong></p>

  <p>
    <img alt="Rust" src="https://img.shields.io/badge/Rust-2024-f74c00?style=for-the-badge&logo=rust&logoColor=white">
    <img alt="Ratatui" src="https://img.shields.io/badge/TUI-Ratatui-22d3ee?style=for-the-badge">
    <img alt="Read only SSH" src="https://img.shields.io/badge/SSH-read--only-69f0ae?style=for-the-badge">
    <img alt="License" src="https://img.shields.io/badge/license-MIT-ffb86b?style=for-the-badge">
  </p>

  <img src="assets/screenshot.png" alt="rktop multi-server dashboard screenshot">
</div>

## Why rktop?

`rktop` is a live terminal dashboard for a small rack, homelab, or fleet of Linux boxes. It collects read-only CPU, RAM, network, disk, uptime, kernel, hostname, and optional CPU temperature metrics, then lays them out as btop-style server cards.

- **Live by default** — running `rktop` opens the live TUI, like `htop`/`btop`.
- **Multi-host over SSH** — Linux hosts are collected with non-interactive key auth.
- **No remote install required** — the SSH collector runs fixed read-only shell probes.
- **Optional hosts** — powered-off boxes can simply disappear instead of breaking the dashboard.
- **Polished terminal UI** — braille history graphs, rule-filled sections, aligned disk rows, and adaptive layout.
- **Storage-friendly** — multiple disks, disk aliases, row limits, and ZFS/TrueNAS-style mount cleanup.

## Quick start

```bash
git clone <repo-url>
cd rktop
scripts/install.sh
rktop config    # create config and open the full-screen setup manager
rktop doctor
rktop
```

`rktop config` creates an intentionally empty first-run config at:

```text
~/.config/server-tui-monitor/config.toml
```

Then it opens the full-screen setup manager where you add servers from `~/.ssh/config`, type a direct `user@host` target, add the local machine, test SSH, run `ssh-copy-id` when needed, reorder entries, and save. `rktop setup` is an alias for the same setup manager; `rktop edit` opens the raw TOML in `$VISUAL`/`$EDITOR` as a fallback.

<p align="center">
  <img src="assets/screenshot_setting.png" alt="rktop setup/config manager screenshot">
</p>

The SSH collector is intentionally read-only and non-interactive. Password prompts are disabled, remote installs/writes are not attempted during monitoring, and every SSH server must already work with key auth. Monitoring and `doctor` use bounded SSH calls with `-n`, `BatchMode=yes`, password and keyboard-interactive auth disabled, one connection attempt, and zero password prompts:

```bash
ssh -o BatchMode=yes server-1 true
```

## Config

First-run config:

```toml
refresh_interval_ms = 1000
```

Add servers from `rktop config`. A minimal local server entry looks like:

```toml
[[servers]]
id = "local"
name = "Local"
source = "local"
```

A minimal SSH server entry looks like:

```toml
[[servers]]
id = "server-1"
name = "Server 1"
source = "ssh"
host = "server-1"
```

Optional servers can be left powered off:

```toml
[[servers]]
id = "optional-server"
name = "Optional Server"
source = "ssh"
host = "optional-server"
optional = true
```

Remote SSH servers are optional by default, so powered-off boxes are hidden instead of rendering broken cards. Set `optional = false` for always-on servers that should show as unavailable when they fail.

Storage hosts can shorten mount labels and reserve more disk rows:

```toml
disk_max_rows = 6
disk_aliases = { "/mnt/tank" = "tank", "/mnt/fast" = "fast" }
```

Server fields:

| field | required | note |
| --- | --- | --- |
| `id` | yes | stable ASCII id, letters/numbers/`-`/`_` |
| `name` | yes | display label in the TUI |
| `source` | yes | `local`/`ssh` for live collection; `proxmox` and `truenas-scale` are preserved config variants for future collectors |
| `host` | for SSH | SSH alias or `user@host` |
| `group` | no | display group; usually same as `name` |
| `role` | no | small role label |
| `enabled` | no | defaults to `true` |
| `optional` | no | SSH/API-style remote entries default to `true`; local entries default to `false`; unreachable optional hosts are hidden |
| `disk_max_rows` | no | per-host disk row limit, capped by the TUI layout |
| `disk_aliases` | no | TOML map for shortening mount labels |


Preserved source variants for future API collectors can use inline-table syntax:

```toml
[[servers]]
id = "pve-api"
name = "PVE API"
source = { type = "proxmox", host = "https://pve.example.invalid:8006" }
enabled = false

[[servers]]
id = "truenas-api"
name = "TrueNAS API"
source = { type = "truenas-scale", host = "https://truenas.example.invalid" }
enabled = false
```

These variants are accepted by config parsing and fixtures, but live collection/doctor intentionally reports them as unsupported until dedicated collectors are implemented.

`refresh_interval_ms` is clamped to `100..60000`. In the TUI, `+`/`=` and `-` adjust it in 100ms steps.

## Commands

```text
server-tui-monitor [--mock|--live] [--snapshot] [--once] [--config PATH]
server-tui-monitor init [--config PATH] [--force|--print]
server-tui-monitor setup [--config PATH]
server-tui-monitor config [--config PATH]
server-tui-monitor edit [--config PATH]
server-tui-monitor doctor [--config PATH]
```

Installed aliases from `scripts/install.sh`:

| command | purpose |
| --- | --- |
| `rktop` | preferred live TUI command |
| `server-tui-monitor` | main binary |
| `stm` | legacy shorthand kept for compatibility |
| `stm-live` | explicit live TUI wrapper |
| `stm-mock` | deterministic mock data |
| `stm-snapshot` | live one-shot text snapshot |

Quit the interactive TUI with `q`, `Ctrl+C`, or `Esc`.

## Diagnostics

```bash
rktop doctor
```

Doctor checks:

- config loads successfully
- at least one server is enabled
- local collector runs on local servers
- SSH aliases are safe and key auth works without prompts
- SSH remote Linux collector can read `/proc`, `df`, memory, network and uptime data

Required server failures make `doctor` exit non-zero. Optional server failures print `warn` and do not fail the command.

## Snapshot mode

```bash
stm-mock --snapshot
rktop --live --snapshot
```

`--snapshot` prints deterministic dashboard text for smoke tests or a read-only live snapshot for enabled hosts. No remote writes, installs, or credential prompts are performed.

## Display details

- CPU/RAM/NET/DISK use btop-style rule-filled section dividers.
- CPU/RAM/NET use full-width dynamic-range braille history graphs.
- CPU temperature is shown when Linux hwmon exposes `coretemp` or `k10temp`; 70°C+ is warning and 85°C+ is critical.
- Disk rows use aligned mount labels, high-contrast block bars, and compact capacity text.
- Disk collection prefers ZFS pool summaries from `zpool list` when available, then falls back to non-pseudo `df` mounts.
- Network values are current throughput (`B/s`, `KiB/s`, `MiB/s`, `GiB/s`), not cumulative boot-time totals.

## Branching strategy

Development uses a lightweight Git Flow style:

- `main` is stable/release-ready.
- `develop` collects the next batch of changes before release merge-back to `main`.
- Work happens on `feat/*`, `fix/*`, `docs/*`, or `chore/*` branches.

See [`docs/branching.md`](docs/branching.md) for the full workflow.

## Development

```bash
cargo fmt --check
cargo check
cargo test
cargo build --release
```

Uninstall local command aliases:

```bash
scripts/install.sh --uninstall
```
