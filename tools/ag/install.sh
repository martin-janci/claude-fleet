#!/usr/bin/env bash
# install.sh — install `ag` for the current user.
#
#   curl -fsSL https://raw.githubusercontent.com/martin-janci/claude-fleet/main/tools/ag/install.sh | bash
#   tools/ag/install.sh --from tools/ag          # from a checkout (fleet provision, tests)
#
# Copies the ag tree to $AG_HOME (~/.local/share/ag), links $AG_BIN_DIR/ag
# (~/.local/bin/ag), writes a starter config when there is none, then runs
# `ag shims` and `ag doctor`. Re-running upgrades in place and keeps the config.
set -eu

FROM=""
DEFAULT=""
while [ $# -gt 0 ]; do
  case $1 in
    --from) FROM=${2:?--from needs a directory}; shift 2 ;;
    --default) DEFAULT=${2:?--default needs a harness}; shift 2 ;;
    -h | --help) echo "usage: install.sh [--from DIR] [--default HARNESS]"; exit 0 ;;
    *) echo "install.sh: unknown argument: $1" >&2; exit 2 ;;
  esac
done

AG_HOME=${AG_HOME:-$HOME/.local/share/ag}
AG_BIN_DIR=${AG_BIN_DIR:-$HOME/.local/bin}
CONFIG=${AG_CONFIG:-${XDG_CONFIG_HOME:-$HOME/.config}/ag/config}
TARBALL=${AG_TARBALL:-https://github.com/martin-janci/claude-fleet/archive/refs/heads/main.tar.gz}

if [ -z "$FROM" ]; then
  tmp=$(mktemp -d "${TMPDIR:-/tmp}/ag-install.XXXXXX")
  trap 'rm -rf "$tmp"' EXIT
  curl -fsSL "$TARBALL" | tar -xz -C "$tmp"
  FROM=$(echo "$tmp"/*/tools/ag)
fi
if ! { [ -f "$FROM/ag" ] && [ -d "$FROM/drivers" ]; }; then
  echo "install.sh: $FROM is not an ag source tree" >&2
  exit 5
fi

# Never delete a directory ag did not create.
if [ -e "$AG_HOME" ] && ! { [ -f "$AG_HOME/ag" ] && [ -d "$AG_HOME/drivers" ]; }; then
  echo "install.sh: $AG_HOME exists and is not an ag install — move it away first" >&2
  exit 5
fi
mkdir -p "$(dirname "$AG_HOME")" "$AG_BIN_DIR"
rm -rf "$AG_HOME.new"
cp -R "$FROM" "$AG_HOME.new"
rm -rf "$AG_HOME"
mv "$AG_HOME.new" "$AG_HOME"
chmod 755 "$AG_HOME/ag"
ln -sfn "$AG_HOME/ag" "$AG_BIN_DIR/ag"
echo "install.sh: installed ag to $AG_HOME"

if [ ! -e "$CONFIG" ]; then
  if [ -z "$DEFAULT" ]; then
    for h in claude codex; do
      if AG_CONFIG=/dev/null "$AG_HOME/ag" which "$h" >/dev/null 2>&1; then DEFAULT=$h; break; fi
    done
  fi
  mkdir -p "$(dirname "$CONFIG")"
  cat >"$CONFIG" <<EOF
# ag config — run \`ag help\` for usage. Values are plain words, no quoting.
default = ${DEFAULT:-claude}
order = claude codex
# Skip permission prompts by default (the same as passing --yolo). Off for safety.
yolo = false

[alias]
# Each entry becomes a command in $AG_BIN_DIR after \`ag shims\`, e.g.:
# cl = claude --yolo
# cx = codex
EOF
  echo "install.sh: wrote $CONFIG"
fi

AG_CONFIG=$CONFIG AG_BIN_DIR=$AG_BIN_DIR "$AG_HOME/ag" shims
AG_CONFIG=$CONFIG AG_BIN_DIR=$AG_BIN_DIR "$AG_HOME/ag" doctor || true
case ":$PATH:" in
  *":$AG_BIN_DIR:"*) ;;
  *) echo "install.sh: add $AG_BIN_DIR to your PATH to use ag" ;;
esac
