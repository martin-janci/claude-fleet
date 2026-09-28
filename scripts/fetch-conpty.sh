#!/usr/bin/env bash
# Fetch Microsoft's ConPTY — conpty.dll and OpenConsole.exe — for the Windows
# bundle, into src-tauri/vendor/conpty/ (gitignored).
#
# WHY. portable-pty loads a `conpty.dll` found next to the app before the one
# built into Windows, and that DLL runs the `OpenConsole.exe` beside it. The
# built-in ConPTY (Windows 10 above all) re-renders the screen and does not
# pass tmux's bracketed-paste and mouse modes through, so a multi-line paste
# submitted at its first line and mouse scrolling did nothing. The current
# OpenConsole passes them through.
#
# WHERE FROM. The Microsoft.Windows.Console.ConPTY NuGet package, built from
# github.com/microsoft/terminal (MIT). Pinned by version AND by the package's
# SHA-256: this runs in the release build, so a changed file must stop it.
# To upgrade: bump VERSION, run with SHA256_OVERRIDE=skip once to see the new
# hash printed, put that hash here, and read the terminal release notes.
#
# Usage: scripts/fetch-conpty.sh [x64|arm64]   (default: this machine's CPU;
# a DLL for another CPU fails to load and portable-pty silently falls back)
# Then build with `--config src-tauri/tauri.conpty.conf.json` (release.yml and
# ci.yml's rust-windows job do). Idempotent; needs curl, sha256sum or shasum,
# and unzip or Python.
set -euo pipefail

VERSION="1.24.260710001"
SHA256="175640566a3b59c4b132070ee96c2c77e5ab7edd2e92732a5eb3610bbf63d90e"
default_arch() {
  case "${PROCESSOR_ARCHITEW6432:-${PROCESSOR_ARCHITECTURE:-$(uname -m 2>/dev/null)}}" in
    ARM64 | arm64 | aarch64) echo arm64 ;;
    *) echo x64 ;;
  esac
}
ARCH="${1:-$(default_arch)}"
case "$ARCH" in
  x64 | arm64) ;;
  *) echo "fetch-conpty.sh: unsupported arch '$ARCH' (x64, arm64)" >&2; exit 2 ;;
esac

here="$(cd "$(dirname "$0")" && pwd)"
dest="$here/../src-tauri/vendor/conpty"
url="https://api.nuget.org/v3-flatcontainer/microsoft.windows.console.conpty/$VERSION/microsoft.windows.console.conpty.$VERSION.nupkg"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
pkg="$work/conpty.nupkg"

curl -fsSL --retry 3 -o "$pkg" "$url"

if command -v sha256sum >/dev/null 2>&1; then
  got="$(sha256sum "$pkg" | cut -d' ' -f1)"
else
  got="$(shasum -a 256 "$pkg" | cut -d' ' -f1)"
fi
if [ "${SHA256_OVERRIDE:-}" = "skip" ]; then
  echo "fetch-conpty.sh: $VERSION has SHA-256 $got (not checked: SHA256_OVERRIDE=skip)" >&2
elif [ "$got" != "$SHA256" ]; then
  echo "fetch-conpty.sh: SHA-256 mismatch for $VERSION: expected $SHA256, got $got" >&2
  exit 1
fi

dll="runtimes/win-$ARCH/native/conpty.dll"
exe="build/native/runtimes/$ARCH/OpenConsole.exe"
mkdir -p "$dest"
if command -v unzip >/dev/null 2>&1; then
  unzip -o -j -q "$pkg" "$dll" "$exe" -d "$dest"
else
  py="$(command -v python3 || command -v python)"
  "$py" - "$pkg" "$dest" "$dll" "$exe" <<'PY'
import os, sys, zipfile
pkg, dest, *members = sys.argv[1:]
with zipfile.ZipFile(pkg) as z:
    for m in members:
        with open(os.path.join(dest, os.path.basename(m)), "wb") as f:
            f.write(z.read(m))
PY
fi

for f in conpty.dll OpenConsole.exe; do
  [ -s "$dest/$f" ] || { echo "fetch-conpty.sh: $f missing after extraction" >&2; exit 1; }
done
echo "fetch-conpty.sh: ConPTY $VERSION ($ARCH) in src-tauri/vendor/conpty/"
