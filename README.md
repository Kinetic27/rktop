<div align="center">
  <img src="assets/logo.svg" alt="rktop" width="520">

  <p><strong>A btop-inspired TUI dashboard for monitoring multiple Linux servers over SSH.</strong></p>

  <p>
    <img alt="Rust" src="https://img.shields.io/badge/Rust-2024-f74c00?style=for-the-badge&logo=rust&logoColor=white">
    <img alt="Ratatui" src="https://img.shields.io/badge/TUI-Ratatui-22d3ee?style=for-the-badge">
    <img alt="SSH" src="https://img.shields.io/badge/SSH-read--only-69f0ae?style=for-the-badge">
    <img alt="License" src="https://img.shields.io/badge/license-MIT-ffb86b?style=for-the-badge">
  </p>

  <img src="assets/screenshot.png" alt="rktop dashboard screenshot">
</div>

## Overview

`rktop` is a live terminal dashboard for monitoring multiple Linux servers from one screen.
It runs locally, collects read-only metrics over SSH, and renders CPU, memory, network, disk, uptime, kernel, hostname, and optional CPU temperature in btop-style cards.

- live by default: `rktop`
- no remote agent or remote install
- optional/offline hosts can be hidden
- btop-inspired braille graphs, bars, and aligned terminal layout
- multiple disks, ZFS pool summaries, aliases, and per-host disk row limits

## Quick start

Pick the install style that matches how much of your machine you want `rktop` to touch:

- **Debian / Ubuntu**: install the `.deb` package.
- **Portable Linux**: extract the tarball and keep config beside the binary.
- **Windows**: use the PowerShell installer or unzip the portable package.
- **Source build**: only needed for development; this is the path that requires Rust.

Release packages do **not** require Rust.

After installing:

```bash
rktop config   # add local/SSH servers, test SSH, reorder, tune disks
rktop doctor   # validate config and SSH health
rktop          # live dashboard
```

## Install

### Debian / Ubuntu (recommended)

```bash
wget https://github.com/Kinetic27/rktop/releases/download/v0.1.5/rktop_0.1.5_amd64.deb
sudo apt install ./rktop_0.1.5_amd64.deb
rktop config
rktop doctor
rktop
```

### Portable release

Use this when you want `rktop` and `config.toml` to stay in one extracted folder.
It does not write to `/usr/bin`, `/etc`, or your user config directory.

```bash
RKTOP_VERSION=v0.1.5
wget "https://github.com/Kinetic27/rktop/releases/download/${RKTOP_VERSION}/rktop_${RKTOP_VERSION#v}_linux_x86_64.tar.gz"
tar -xzf "rktop_${RKTOP_VERSION#v}_linux_x86_64.tar.gz"
cd rktop
./rktop config
./rktop doctor
./rktop
```

### Windows

Native Windows builds are for running the dashboard from Windows while monitoring Linux SSH hosts.
In other words, native `rktop.exe` can monitor Linux SSH hosts.
Local Windows metrics and Windows remote hosts are not implemented yet.

One-line user PATH installer:

```powershell
powershell -ExecutionPolicy ByPass -c "irm https://raw.githubusercontent.com/Kinetic27/rktop/main/scripts/install.ps1 | iex"
```

Portable zip, if you do not want PATH or installer state touched:

```powershell
$Version = "v0.1.5"
Invoke-WebRequest "https://github.com/Kinetic27/rktop/releases/download/$Version/rktop_${Version}_windows_x86_64.zip" -OutFile rktop.zip
Expand-Archive .\rktop.zip -DestinationPath . -Force
cd .\rktop
.\rktop.exe config
.\rktop.exe doctor
.\rktop.exe
```

Installer knobs:

```powershell
$env:RKTOP_VERSION="v0.1.5"        # pin a release
$env:RKTOP_INSTALL_DIR="E:\rktop"  # custom install dir
$env:RKTOP_SKIP_PATH="1"           # do not edit user PATH
$env:RKTOP_NON_INTERACTIVE="1"     # CI/unattended mode
```

### Source build

Use this for development. Rust is required because it builds from source.
The install stays inside the clone under `./.rktop/`.

```bash
git clone https://github.com/Kinetic27/rktop.git
cd rktop
scripts/install.sh
./.rktop/bin/rktop config
./.rktop/bin/rktop
```

## Configuration

```bash
rktop config
```

`rktop config` opens the TUI config manager. It can add SSH hosts, add the local Linux machine, test SSH, show `ssh-copy-id` guidance, reorder hosts, mark hosts optional, and edit disk display options.

<p align="center">
  <img src="assets/screenshot_setting.png" alt="rktop config manager screenshot">
</p>

Remote hosts need key-based SSH:

```bash
ssh -o BatchMode=yes user@host true
```

No remote writes, installs, or credential prompts are performed. SSH probes and metric collection have bounded timeouts, so broken SSH commands do not hang the app indefinitely.

Useful commands:

```bash
rktop doctor
rktop --live --snapshot
rktop edit
```

Config lookup order:

```text
--config PATH
$RKTOP_CONFIG
./config.toml beside the executable
~/.config/rktop/config.toml or $XDG_CONFIG_HOME/rktop/config.toml
/etc/rktop/config.toml on Linux
legacy ~/.config/server-tui-monitor/config.toml
```

Minimal config:

```toml
refresh_interval_ms = 1000

[[servers]]
id = "local"
name = "Local"
source = "local"

[[servers]]
id = "server-1"
name = "Server 1"
source = "ssh"
host = "server-1"
optional = false
```

## Support matrix

### Running rktop

| Environment | Status |
| --- | --- |
| Linux x86_64 | supported |
| Debian / Ubuntu | supported with `.deb` |
| Windows 10/11 | supported for monitoring Linux SSH hosts |
| macOS | not supported/tested |

### Monitored hosts

| Target | Status |
| --- | --- |
| Linux over SSH | supported |
| Local Linux machine | supported |
| Proxmox host/VM over SSH | supported |
| Linux NAS / TrueNAS SCALE over SSH | supported when Linux shell access works |
| TrueNAS CORE / FreeBSD / BSD | not supported |
| Windows remote | not supported |

Remote Linux hosts should provide a POSIX shell, `/proc`, `df`, `awk`, `grep`, `hostname`, `uname`, and `base64` when monitored from Windows.


## Refresh behavior

`refresh_interval_ms` is clamped to `100..60000`. In the TUI, `+`/`=` and `-` adjust it in 100ms steps. Unix-like machines running `rktop` use SSH `ControlMaster=auto` and `ControlPersist=10m` for faster repeated polling. Windows OpenSSH does not use those Unix socket multiplexing options. Optional hosts that fail are skipped briefly before retrying.

## Controls

| Key | Action |
| --- | --- |
| `q`, `Esc`, `Ctrl+C` | quit |
| `+`, `=` | faster refresh |
| `-` | slower refresh |

## Commands

```text
rktop [--mock|--live] [--snapshot] [--once] [--config PATH]
rktop init [--config PATH] [--force|--print]
rktop config [--config PATH]
rktop setup [--config PATH]
rktop edit [--config PATH]
rktop doctor [--config PATH]
```

Legacy compatibility wrappers are still packaged: `stm`, `stm-live`, `stm-mock`, and `stm-snapshot`.

## Development

Tests do not open live SSH connections or require remote credentials. Live SSH/manual checks are intentionally outside automated tests.


```bash
cargo fmt --check
cargo check --locked
cargo test --locked
cargo build --release --locked
```

Build a local Debian package:

```bash
scripts/build-deb.sh
sudo apt install ./dist/rktop_0.1.5_amd64.deb
```

Development uses a lightweight Git Flow style. `main` is stable/release-ready. `develop` collects the next batch of changes before release merge-back to `main`, and work happens on topic branches. See [`docs/branching.md`](docs/branching.md).
