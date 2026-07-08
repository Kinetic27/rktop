#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/install.sh [--portable] [--no-build] [--uninstall]

Builds a clone-local portable rktop install for development/testing.
Rust is required because this script builds from source.

Output stays inside the repository clone:
  ./.rktop/bin/rktop
  ./.rktop/bin/stm
  ./.rktop/bin/stm-live
  ./.rktop/bin/stm-mock
  ./.rktop/bin/stm-snapshot
  ./.rktop/config.toml

Options:
  --portable   Accepted for clarity; portable mode is the only install mode
  --no-build   Do not run cargo build --release before installing
  --uninstall  Remove ./.rktop/bin command files; keep ./.rktop/config.toml
USAGE
}

build=true
uninstall=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    --portable)
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
portable_root="${repo_root}/.rktop"
bin_dir="${portable_root}/bin"
config_file="${portable_root}/config.toml"
bin_names=(rktop rktop-bin stm stm-live stm-mock stm-snapshot)

if [[ "$uninstall" == true ]]; then
  for name in "${bin_names[@]}"; do
    rm -f "${bin_dir}/${name}"
  done
  echo "Removed rktop portable commands from ${bin_dir}"
  echo "Portable config was left in place: ${config_file}"
  exit 0
fi

mkdir -p "$bin_dir"

if [[ "$build" == true ]]; then
  export PATH="${HOME}/.cargo/bin:${PATH}"
  (cd "$repo_root" && cargo build --release --locked)
fi

binary="${repo_root}/target/release/rktop"
if [[ ! -x "$binary" ]]; then
  echo "Release binary not found or not executable: ${binary}" >&2
  echo "Run: PATH=\$HOME/.cargo/bin:\$PATH cargo build --release --locked" >&2
  exit 1
fi

install -m 0755 "$binary" "${bin_dir}/rktop-bin"

if [[ ! -f "$config_file" ]]; then
  cp "${repo_root}/config/rktop.example.toml" "$config_file"
fi

write_wrapper() {
  local name="$1"
  shift || true
  local extra_args="$*"
  local path="${bin_dir}/${name}"
  cat > "$path" <<WRAP
#!/usr/bin/env bash
set -euo pipefail
script_dir="\$(CDPATH= cd -- "\$(dirname -- "\${BASH_SOURCE[0]}")" && pwd)"
portable_root="\$(CDPATH= cd -- "\${script_dir}/.." && pwd)"
export RKTOP_CONFIG="\${RKTOP_CONFIG:-\${portable_root}/config.toml}"
exec "\${script_dir}/rktop-bin" ${extra_args} "\$@"
WRAP
  chmod 0755 "$path"
}

write_wrapper rktop
write_wrapper stm
write_wrapper stm-live --live
write_wrapper stm-mock --mock
write_wrapper stm-snapshot --live --snapshot

cat <<EOF2
Installed portable rktop into ${portable_root}:
  ./.rktop/bin/rktop
  ./.rktop/config.toml

First run:
  ./.rktop/bin/rktop config
  ./.rktop/bin/rktop doctor
  ./.rktop/bin/rktop

Helpers:
  ./.rktop/bin/stm-snapshot
  ./.rktop/bin/stm-mock --snapshot
EOF2
