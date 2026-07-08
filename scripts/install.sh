#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/install.sh [--prefix DIR] [--no-build] [--uninstall]

Installs Server TUI Monitor commands into ~/.local/bin by default:
  server-tui-monitor   main binary
  rktop                preferred alias wrapper for default live server-tui-monitor
  stm                  legacy alias wrapper for default live server-tui-monitor
  stm-live             explicit live wrapper for server-tui-monitor --live
  stm-mock             alias wrapper for server-tui-monitor --mock
  stm-snapshot         alias wrapper for server-tui-monitor --live --snapshot

First run after install:
  rktop init           create ~/.config/server-tui-monitor/config.toml
  rktop config         open config in $VISUAL/$EDITOR
  rktop doctor         verify config, SSH aliases, and key auth

Options:
  --prefix DIR   Install commands into DIR instead of ~/.local/bin
  --no-build     Do not run cargo build --release before installing
  --uninstall    Remove installed commands from the prefix
USAGE
}

prefix="${HOME}/.local/bin"
build=true
uninstall=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    --prefix)
      prefix="${2:?--prefix requires a directory}"
      shift 2
      ;;
    --no-build)
      build=false
      shift
      ;;
    --uninstall)
      uninstall=true
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bin_names=(server-tui-monitor rktop stm stm-live stm-mock stm-snapshot)

if [[ "$uninstall" == true ]]; then
  for name in "${bin_names[@]}"; do
    rm -f "${prefix}/${name}"
  done
  echo "Removed Server TUI Monitor commands from ${prefix}"
  exit 0
fi

mkdir -p "$prefix"

if [[ "$build" == true ]]; then
  export PATH="${HOME}/.cargo/bin:${PATH}"
  (cd "$repo_root" && cargo build --release)
fi

binary="${repo_root}/target/release/server-tui-monitor"
if [[ ! -x "$binary" ]]; then
  echo "Release binary not found or not executable: ${binary}" >&2
  echo "Run: PATH=\$HOME/.cargo/bin:\$PATH cargo build --release" >&2
  exit 1
fi

install -m 0755 "$binary" "${prefix}/server-tui-monitor"

cat > "${prefix}/rktop" <<'WRAP'
#!/usr/bin/env bash
exec server-tui-monitor "$@"
WRAP

cat > "${prefix}/stm" <<'WRAP'
#!/usr/bin/env bash
exec server-tui-monitor "$@"
WRAP

cat > "${prefix}/stm-live" <<'WRAP'
#!/usr/bin/env bash
exec server-tui-monitor --live "$@"
WRAP

cat > "${prefix}/stm-mock" <<'WRAP'
#!/usr/bin/env bash
exec server-tui-monitor --mock "$@"
WRAP

cat > "${prefix}/stm-snapshot" <<'WRAP'
#!/usr/bin/env bash
exec server-tui-monitor --live --snapshot "$@"
WRAP

chmod 0755 "${prefix}/rktop" "${prefix}/stm" "${prefix}/stm-live" "${prefix}/stm-mock" "${prefix}/stm-snapshot"

cat <<EOF2
Installed Server TUI Monitor commands into ${prefix}:
  server-tui-monitor
  rktop
  stm
  stm-live
  stm-mock
  stm-snapshot

First run:
  rktop init
  rktop config
  rktop doctor

Try:
  rktop
  stm-snapshot
  stm-mock --snapshot
EOF2
