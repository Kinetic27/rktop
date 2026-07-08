#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/install.sh [--prefix DIR] [--portable] [--no-build] [--uninstall]

Installs rktop commands into ~/.local/bin by default:
  server-tui-monitor   main binary
  rktop                preferred alias wrapper for default live server-tui-monitor
  stm                  legacy alias wrapper for default live server-tui-monitor
  stm-live             explicit live wrapper for server-tui-monitor --live
  stm-mock             alias wrapper for server-tui-monitor --mock
  stm-snapshot         alias wrapper for server-tui-monitor --live --snapshot

Portable clone-local install:
  scripts/install.sh --portable
  ./.rktop/bin/rktop config
  ./.rktop/bin/rktop

Portable mode keeps both install files and config inside the clone:
  ./.rktop/bin/rktop
  ./.rktop/config.toml

First run after normal install:
  rktop init           create ~/.config/rktop/config.toml
  rktop config         open the full-screen setup manager
  rktop doctor         verify config, SSH aliases, and key auth

Options:
  --prefix DIR   Install commands into DIR instead of ~/.local/bin
  --portable     Install into ./.rktop/bin and default config to ./.rktop/config.toml
  --no-build     Do not run cargo build --release before installing
  --uninstall    Remove installed commands from the selected prefix
USAGE
}

prefix="${HOME}/.local/bin"
build=true
uninstall=false
portable=false
prefix_was_set=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    --prefix)
      prefix="${2:?--prefix requires a directory}"
      prefix_was_set=true
      shift 2
      ;;
    --portable)
      portable=true
      shift
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
if [[ "$portable" == true ]]; then
  if [[ "$prefix_was_set" == true ]]; then
    echo "--portable cannot be combined with --prefix" >&2
    exit 2
  fi
  prefix="${repo_root}/.rktop/bin"
fi
portable_config="${repo_root}/.rktop/config.toml"
bin_names=(server-tui-monitor rktop stm stm-live stm-mock stm-snapshot)

if [[ "$uninstall" == true ]]; then
  for name in "${bin_names[@]}"; do
    rm -f "${prefix}/${name}"
  done
  echo "Removed rktop commands from ${prefix}"
  if [[ "$portable" == true ]]; then
    echo "Portable config was left in place: ${portable_config}"
  fi
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

write_wrapper() {
  local name="$1"
  shift || true
  local extra_args="$*"
  local path="${prefix}/${name}"

  if [[ "$portable" == true ]]; then
    cat > "$path" <<WRAP
#!/usr/bin/env bash
set -euo pipefail
script_dir="\$(CDPATH= cd -- "\$(dirname -- "\${BASH_SOURCE[0]}")" && pwd)"
portable_root="\$(CDPATH= cd -- "\${script_dir}/.." && pwd)"
export RKTOP_CONFIG="\${RKTOP_CONFIG:-\${portable_root}/config.toml}"
exec "\${script_dir}/server-tui-monitor" ${extra_args} "\$@"
WRAP
  else
    cat > "$path" <<WRAP
#!/usr/bin/env bash
exec server-tui-monitor ${extra_args} "\$@"
WRAP
  fi
}

write_wrapper rktop
write_wrapper stm
write_wrapper stm-live --live
write_wrapper stm-mock --mock
write_wrapper stm-snapshot --live --snapshot

chmod 0755 "${prefix}/rktop" "${prefix}/stm" "${prefix}/stm-live" "${prefix}/stm-mock" "${prefix}/stm-snapshot"

cat <<EOF2
Installed rktop commands into ${prefix}:
  server-tui-monitor
  rktop
  stm
  stm-live
  stm-mock
  stm-snapshot

First run:
EOF2

if [[ "$portable" == true ]]; then
  cat <<EOF2
  ./.rktop/bin/rktop init
  ./.rktop/bin/rktop config
  ./.rktop/bin/rktop doctor

Portable config:
  ${portable_config}

Try:
  ./.rktop/bin/rktop
  ./.rktop/bin/stm-snapshot
  ./.rktop/bin/stm-mock --snapshot
EOF2
else
  cat <<'EOF2'
  rktop init
  rktop config
  rktop doctor

Try:
  rktop
  stm-snapshot
  stm-mock --snapshot
EOF2
fi
