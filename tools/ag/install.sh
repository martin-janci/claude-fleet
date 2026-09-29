#!/usr/bin/env bash
# install.sh — install `ag` for the current user.
#
#   curl -fsSL https://raw.githubusercontent.com/martin-janci/claude-fleet/main/tools/ag/install.sh | bash
#   tools/ag/install.sh --from tools/ag          # from a checkout (fleet provision, tests)
#
# Copies the ag tree to $AG_HOME (~/.local/share/ag), links $AG_BIN_DIR/ag
# (~/.local/bin/ag), writes a starter config when there is none, then runs
# `ag shims` and `ag doctor`. Re-running upgrades in place and keeps the config.
#
# The whole body lives in main(), called only at the very last line: a
# `curl | bash` download truncated mid-script then defines an incomplete
# main and never calls it, instead of running a half-written script.
set -euo pipefail

main() {
  local FROM="" DEFAULT=""
  while [ $# -gt 0 ]; do
    case $1 in
      --from) FROM=${2:?--from needs a directory}; shift 2 ;;
      --default) DEFAULT=${2:?--default needs a harness}; shift 2 ;;
      -h | --help) echo "usage: install.sh [--from DIR] [--default HARNESS]"; return 0 ;;
      *) echo "install.sh: unknown argument: $1" >&2; return 2 ;;
    esac
  done

  # Each is `local x=$expr` in one statement, not a bare `local x` followed by
  # a separate assignment: bash evaluates a bare `local x` by shadowing first
  # (wiping any exported value) and only then would the assignment run,
  # silently discarding a caller-set AG_HOME/AG_BIN_DIR/AG_CONFIG/AG_TARBALL.
  local AG_HOME=${AG_HOME:-$HOME/.local/share/ag}
  local AG_BIN_DIR=${AG_BIN_DIR:-$HOME/.local/bin}
  local CONFIG=${AG_CONFIG:-${XDG_CONFIG_HOME:-$HOME/.config}/ag/config}
  local TARBALL=${AG_TARBALL:-https://github.com/martin-janci/claude-fleet/archive/refs/heads/main.tar.gz}

  case $AG_HOME in
    /*) ;;
    *) echo "install.sh: AG_HOME must be an absolute path (got: $AG_HOME)" >&2; return 2 ;;
  esac
  case $AG_BIN_DIR in
    /*) ;;
    *) echo "install.sh: AG_BIN_DIR must be an absolute path (got: $AG_BIN_DIR)" >&2; return 2 ;;
  esac

  if [ -z "$FROM" ]; then
    local tmp
    tmp=$(mktemp -d "${TMPDIR:-/tmp}/ag-install.XXXXXX")
    trap 'rm -rf "$tmp"' RETURN
    if ! curl -fsSL "$TARBALL" | tar -xz -C "$tmp"; then
      echo "install.sh: download of $TARBALL failed or was incomplete" >&2
      return 5
    fi
    FROM=$(echo "$tmp"/*/tools/ag)
  fi
  if ! { [ -f "$FROM/ag" ] && [ -d "$FROM/drivers" ]; }; then
    echo "install.sh: $FROM is not an ag source tree" >&2
    return 5
  fi

  # Never delete a directory ag did not create.
  if [ -e "$AG_HOME" ] && ! { [ -f "$AG_HOME/ag" ] && [ -d "$AG_HOME/drivers" ]; }; then
    echo "install.sh: $AG_HOME exists and is not an ag install — move it away first" >&2
    return 5
  fi
  mkdir -p "$(dirname "$AG_HOME")" "$AG_BIN_DIR"
  rm -rf "$AG_HOME.new"
  cp -R "$FROM" "$AG_HOME.new"
  rm -rf "$AG_HOME"
  mv "$AG_HOME.new" "$AG_HOME"
  chmod 755 "$AG_HOME/ag"
  echo "install.sh: installed ag to $AG_HOME"

  # Never replace a file at $AG_BIN_DIR/ag that ag itself did not create —
  # e.g. the Silver Searcher's own `ag`. Only a bare symlink already pointing
  # at this $AG_HOME/ag is ours to repoint.
  local bin_ag=$AG_BIN_DIR/ag
  if [ -e "$bin_ag" ] || [ -L "$bin_ag" ]; then
    if [ -L "$bin_ag" ] && [ "$(readlink "$bin_ag")" = "$AG_HOME/ag" ]; then
      ln -sfn "$AG_HOME/ag" "$bin_ag"
    else
      echo "install.sh: $bin_ag already exists and is not ag's own symlink — leaving it alone." >&2
      echo "install.sh: move it aside, or set AG_BIN_DIR to install ag somewhere else on PATH." >&2
      return 5
    fi
  else
    ln -sfn "$AG_HOME/ag" "$bin_ag"
  fi

  if [ ! -e "$CONFIG" ]; then
    if [ -z "$DEFAULT" ]; then
      local h
      for h in claude codex; do
        if AG_CONFIG=/dev/null "$AG_HOME/ag" which "$h" </dev/null >/dev/null 2>&1; then DEFAULT=$h; break; fi
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

  # `ag shims` failing (e.g. a foreign file blocking one alias) is not fatal
  # to the install: the core install (tree + symlink + config) already
  # succeeded, so still run doctor and the PATH hint, and still exit 0.
  if ! AG_CONFIG=$CONFIG AG_BIN_DIR=$AG_BIN_DIR "$AG_HOME/ag" shims </dev/null; then
    echo "install.sh: ag shims reported a problem above — install continues; fix it and re-run \`ag shims\`" >&2
  fi
  AG_CONFIG=$CONFIG AG_BIN_DIR=$AG_BIN_DIR "$AG_HOME/ag" doctor </dev/null || true
  case ":$PATH:" in
    *":$AG_BIN_DIR:"*) ;;
    *) echo "install.sh: add $AG_BIN_DIR to your PATH to use ag" ;;
  esac
  return 0
}

main "$@"
