#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/build-deb.sh [--no-build] [--out-dir DIR]

Builds a Debian package for rktop without requiring cargo-deb/fpm.
Output:
  dist/rktop_<version>_<arch>.deb

Environment:
  VERSION       Override package version. Defaults to Cargo.toml package.version.
  TARGET_ARCH   Override Debian architecture. Defaults from uname -m.
USAGE
}

build=true
out_dir="dist"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-build)
      build=false
      shift
      ;;
    --out-dir)
      out_dir="${2:?--out-dir requires a directory}"
      shift 2
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
cd "$repo_root"

version="${VERSION:-$(awk -F ' *= *' '/^version *=/ {gsub(/"/, "", $2); print $2; exit}' Cargo.toml)}"
if [[ -z "$version" ]]; then
  echo "Could not determine package version from Cargo.toml" >&2
  exit 1
fi

case "${TARGET_ARCH:-$(uname -m)}" in
  x86_64|amd64) deb_arch="amd64" ;;
  aarch64|arm64) deb_arch="arm64" ;;
  armv7l|armhf) deb_arch="armhf" ;;
  *)
    echo "Unsupported architecture: ${TARGET_ARCH:-$(uname -m)}" >&2
    exit 1
    ;;
esac

if [[ "$build" == true ]]; then
  export PATH="${HOME}/.cargo/bin:${PATH}"
  cargo build --release --locked
fi

binary="target/release/rktop"
if [[ ! -x "$binary" ]]; then
  echo "Release binary not found or not executable: $binary" >&2
  exit 1
fi

pkg_name="rktop"
stage="target/deb/${pkg_name}_${version}_${deb_arch}"
rm -rf "$stage"
mkdir -p \
  "$stage/DEBIAN" \
  "$stage/usr/bin" \
  "$stage/usr/share/doc/$pkg_name" \
  "$stage/usr/share/licenses/$pkg_name"

install -m 0755 "$binary" "$stage/usr/bin/rktop"
ln -s rktop "$stage/usr/bin/stm"

cat > "$stage/usr/bin/stm-live" <<'WRAP'
#!/usr/bin/env sh
exec rktop --live "$@"
WRAP
cat > "$stage/usr/bin/stm-mock" <<'WRAP'
#!/usr/bin/env sh
exec rktop --mock "$@"
WRAP
cat > "$stage/usr/bin/stm-snapshot" <<'WRAP'
#!/usr/bin/env sh
exec rktop --live --snapshot "$@"
WRAP
chmod 0755 "$stage/usr/bin/stm-live" "$stage/usr/bin/stm-mock" "$stage/usr/bin/stm-snapshot"

install -m 0644 README.md "$stage/usr/share/doc/$pkg_name/README.md"
install -m 0644 LICENSE "$stage/usr/share/licenses/$pkg_name/LICENSE"
cat > "$stage/usr/share/doc/$pkg_name/copyright" <<'COPYRIGHT'
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: rktop
Source: https://github.com/Kinetic27/rktop

Files: *
Copyright: 2026 Kinetic27
License: MIT

License: MIT
 Permission is hereby granted, free of charge, to any person obtaining a copy
 of this software and associated documentation files (the "Software"), to deal
 in the Software without restriction, including without limitation the rights
 to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 copies of the Software, and to permit persons to whom the Software is
 furnished to do so, subject to the following conditions:
 .
 The above copyright notice and this permission notice shall be included in all
 copies or substantial portions of the Software.
 .
 THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
 SOFTWARE.
COPYRIGHT

du_kib="$(du -sk "$stage/usr" | awk '{print $1}')"
cat > "$stage/DEBIAN/control" <<CONTROL
Package: $pkg_name
Version: $version
Section: utils
Priority: optional
Architecture: $deb_arch
Maintainer: Kinetic27 <noreply@github.com>
Depends: libc6, openssh-client
Installed-Size: $du_kib
Homepage: https://github.com/Kinetic27/rktop
Description: btop-inspired TUI dashboard for multiple Linux servers
 rktop is a live terminal dashboard for monitoring several Linux hosts over
 SSH. It collects read-only CPU, RAM, network, disk, uptime, hostname, kernel,
 and optional temperature metrics without installing agents on remote hosts.
CONTROL

mkdir -p "$out_dir"
deb_path="$out_dir/${pkg_name}_${version}_${deb_arch}.deb"
dpkg-deb --build --root-owner-group "$stage" "$deb_path" >/dev/null
sha256sum "$deb_path" > "$deb_path.sha256"

echo "Built $deb_path"
echo "Built $deb_path.sha256"
