#!/usr/bin/env bash
# install.sh — install `ag` for the current user.
#
#   curl -fsSL https://raw.githubusercontent.com/martin-janci/claude-fleet/main/tools/ag/install.sh | bash
#   tools/ag/install.sh --from tools/ag          # from a checkout (fleet provision, tests)
#   tools/ag/install.sh --alias 'cl=claude --yolo'  # add an alias (repeatable)
#
# Copies the ag tree to $AG_HOME (~/.local/share/ag), links $AG_BIN_DIR/ag
# (~/.local/bin/ag), writes a starter config when there is none, then runs
# `ag shims` and `ag doctor`. Re-running upgrades in place and keeps the config.
# --alias NAME=VALUE adds the alias to [alias] unless the config already has
# that name (a user's existing value is kept) or NAME is already a command of
# the user's (a foreign $AG_BIN_DIR/NAME, or another NAME on PATH); a
# symlinked config is never edited (it is managed elsewhere). Repeatable.
#
# The whole body lives in main(), called only at the very last line: a
# `curl | bash` download truncated mid-script then defines an incomplete
# main and never calls it, instead of running a half-written script.
set -euo pipefail

main() {
  local FROM="" DEFAULT=""
  local ALIASES=()
  while [ $# -gt 0 ]; do
    case $1 in
      --from) FROM=${2:?--from needs a directory}; shift 2 ;;
      --default) DEFAULT=${2:?--default needs a harness}; shift 2 ;;
      --alias) ALIASES+=("${2:?--alias needs NAME=VALUE}"); shift 2 ;;
      -h | --help) echo "usage: install.sh [--from DIR] [--default HARNESS] [--alias NAME=VALUE]..."; return 0 ;;
      *) echo "install.sh: unknown argument: $1" >&2; return 2 ;;
    esac
  done

  # Validate every --alias before touching anything (same rules as `ag shims`).
  local a name val
  for a in ${ALIASES[@]+"${ALIASES[@]}"}; do
    case $a in
      *=*) ;;
      *) echo "install.sh: --alias needs NAME=VALUE (got: $a)" >&2; return 2 ;;
    esac
    name=${a%%=*}
    val=${a#*=}
    case $name in
      '' | *[!A-Za-z0-9._-]*) echo "install.sh: invalid alias name: $name" >&2; return 2 ;;
    esac
    case $val in
      '' | *[!A-Za-z0-9\ ._=/:@%+-]*) echo "install.sh: alias $name: value must be plain words" >&2; return 2 ;;
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
  # `lib/` too: without it the installed tree has no `ag_launch`, which `ag`
  # itself now refuses rather than mistaking for an unimplemented feature. The
  # pane fallback (`tmux::CL_FALLBACK`) already requires all three before it
  # will use `ag`, so an install missing one is an install nothing uses.
  if ! { [ -f "$FROM/ag" ] && [ -d "$FROM/drivers" ] && [ -d "$FROM/lib" ]; }; then
    echo "install.sh: $FROM is not an ag source tree (needs ag, drivers/ and lib/)" >&2
    return 5
  fi

  # Never delete a directory ag did not create. `ag` + `drivers/` alone is NOT
  # enough to prove that: an ag SOURCE CHECKOUT has both, and $AG_HOME is
  # caller-settable (and fleet runs this through `bash -lc`, so an
  # `export AG_HOME=...` in the host's login profile reaches it) — so that test
  # let `rm -rf "$AG_HOME"` below destroy a developer's checkout, .git and
  # uncommitted work included, while exiting 0.
  #
  # Two ways to be ours, and nothing else is:
  #   - it carries fleet's `.fleet-managed` marker, which `cp -R` copies in
  #     from the staged source tree; or
  #   - every entry in it also exists in $FROM, i.e. it holds only the files
  #     this installer would itself have written. Deriving the set from $FROM
  #     keeps the rule correct as the ag file set changes, and it preserves the
  #     manual-install upgrade path for someone who never used fleet.
  if [ -e "$AG_HOME" ]; then
    if ! { [ -f "$AG_HOME/ag" ] && [ -d "$AG_HOME/drivers" ]; }; then
      echo "install.sh: $AG_HOME exists and is not an ag install — move it away first" >&2
      return 5
    fi
    if [ ! -f "$AG_HOME/.fleet-managed" ]; then
      local stray=
      local entry base
      for entry in "$AG_HOME"/* "$AG_HOME"/.[!.]* "$AG_HOME"/..?*; do
        [ -e "$entry" ] || [ -L "$entry" ] || continue
        base=${entry##*/}
        [ "$base" = ".fleet-managed" ] && continue
        if [ ! -e "$FROM/$base" ] && [ ! -L "$FROM/$base" ]; then
          stray=$base
          break
        fi
      done
      if [ -n "$stray" ]; then
        echo "install.sh: $AG_HOME holds $stray, which ag did not install — refusing to replace it." >&2
        echo "install.sh: that looks like your own checkout or directory. Move it aside, or set" >&2
        echo "install.sh: AG_HOME to a path ag may own." >&2
        return 5
      fi
    fi
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
    # Never shadow an `ag` the user already has ELSEWHERE on PATH either — the
    # Silver Searcher is the common one. The alias arm below makes exactly this
    # check with `command -v`; this arm only looked at $AG_BIN_DIR, so a fresh
    # install would drop a symlink that either shadows their search tool (when
    # $AG_BIN_DIR sorts earlier on PATH) or never runs (when it sorts later) —
    # silently, either way.
    #
    # Refusing costs fleet nothing: the tree is installed and `tmux::CL_FALLBACK`
    # invokes $AG_HOME/ag by absolute path, so panes still launch through it.
    local found_ag
    found_ag=$(command -v ag 2>/dev/null || true)
    if [ -n "$found_ag" ] && ! [ "$found_ag" -ef "$AG_HOME/ag" ]; then
      echo "install.sh: ag is already a command ($found_ag) — kept yours, no symlink made." >&2
      echo "install.sh: fleet still uses $AG_HOME/ag directly. To put ag on PATH, move theirs" >&2
      echo "install.sh: aside or set AG_BIN_DIR to a directory earlier on PATH." >&2
      return 5
    fi
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

  # --alias: add each alias unless the config already defines that name,
  # the user already has a command of that name, or the config is a symlink.
  local shim found
  for a in ${ALIASES[@]+"${ALIASES[@]}"}; do
    name=${a%%=*}
    val=${a#*=}
    shim=$AG_BIN_DIR/$name
    if awk -v n="$name" '
        /^[[:space:]]*[#;]/ { next }
        /^[[:space:]]*\[/ { s = $0; gsub(/^[[:space:]]*\[|\][[:space:]]*$/, "", s); sec = s; next }
        sec == "alias" {
          i = index($0, "="); if (i == 0) next
          k = substr($0, 1, i - 1); gsub(/^[[:space:]]+|[[:space:]]+$/, "", k)
          if (k == n) found = 1
        }
        END { exit found ? 0 : 1 }' "$CONFIG"; then
      echo "install.sh: alias $name is already set in $CONFIG — kept"
      continue
    fi
    # Never shadow a user's own command: a file ag did not generate at
    # $AG_BIN_DIR/NAME, or NAME resolving on PATH to anything but our shim.
    if [ -e "$shim" ] && ! grep -q '^# generated by ag-shims' "$shim" 2>/dev/null; then
      echo "install.sh: $shim exists and was not generated by ag — kept yours; alias $name not added"
      continue
    fi
    found=$(command -v "$name" 2>/dev/null || true)
    if [ -n "$found" ] && ! [ "$found" -ef "$shim" ]; then
      echo "install.sh: $name is already a command ($found) — kept yours; alias $name not added"
      continue
    fi
    if [ -L "$CONFIG" ]; then
      echo "install.sh: $CONFIG is managed elsewhere (symlink): add \`$name = $val\` under [alias] there"
      continue
    fi
    if grep -q '^[[:space:]]*\[alias\][[:space:]]*$' "$CONFIG"; then
      # Written back THROUGH the existing file, not over it. A tmp + mv replaces
      # the inode, so rename(2) left the config with the umask default instead of
      # the mode its owner chose -- and dropped any hard link to it. cat > keeps
      # the inode, the mode and the owner. The non-empty test is what makes that
      # safe: a cat that truncates and then fails would otherwise leave nothing.
      if ! {
        awk -v line="$name = $val" '
          { print }
          /^[[:space:]]*\[alias\][[:space:]]*$/ && !done { print line; done = 1 }' "$CONFIG" >"$CONFIG.tmp" &&
          [ -s "$CONFIG.tmp" ] &&
          cat "$CONFIG.tmp" >"$CONFIG" &&
          rm -f "$CONFIG.tmp"
      }; then
        rm -f "$CONFIG.tmp"
        echo "install.sh: cannot add alias $name to $CONFIG" >&2
        return 5
      fi
    elif ! printf '\n[alias]\n%s = %s\n' "$name" "$val" >>"$CONFIG"; then
      echo "install.sh: cannot add alias $name to $CONFIG" >&2
      return 5
    fi
    echo "install.sh: added alias $name = $val"
  done

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
