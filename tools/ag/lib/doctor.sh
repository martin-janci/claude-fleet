# shellcheck shell=bash
# `ag doctor`: is this machine ready to launch agents? One line per check;
# every FAIL ends with the exact fix. Exit 1 if anything failed.
# `ag install <h>`: print the harness's official install command.

ag_cmd_doctor() {
  local bad=0 h p v d dir name f rp t line ag_path
  for h in $(ag_harnesses); do
    if p=$(ag_bin "$h"); then
      v=$("$p" --version 2>/dev/null | head -n 1)
      echo "ok    $h: $p${v:+ ($v)}"
    else
      echo "--    $h: not installed (install: $(ag_load_driver "$h" && drv_install_hint))"
    fi
  done
  if d=$(ag_resolve_harness ""); then
    if ! ag_is_harness "$d"; then
      echo "FAIL  default harness '$d' is unknown — fix: set default to one of: $(ag_harnesses | tr '\n' ' ') in $(ag_config_file)"
      bad=1
    elif ag_bin "$d" >/dev/null; then
      echo "ok    default harness: $d"
    else
      echo "FAIL  default harness '$d' is not usable — fix: ag install $d (or change default in $(ag_config_file))"
      bad=1
    fi
  else
    echo "FAIL  no harness installed — fix: ag install claude"
    bad=1
  fi
  dir=$(ag_bin_dir)
  case ":$PATH:" in
    *":$dir:"*) echo "ok    $dir is on PATH" ;;
    *)
      echo "FAIL  $dir is not on PATH — fix: add  export PATH=\"$dir:\$PATH\"  to ~/.profile and your shell rc"
      bad=1 ;;
  esac
  # Another `ag` (e.g. the Silver Searcher) earlier on PATH would silently
  # shadow this install every time the user types `ag`.
  if p=$(command -v ag 2>/dev/null); then
    rp=$p
    while [ -L "$rp" ]; do
      t=$(readlink "$rp")
      case $t in /*) rp=$t ;; *) rp=$(dirname "$rp")/$t ;; esac
    done
    # shellcheck disable=SC1007  # intentional: clear CDPATH for this one `cd`
    rp=$(CDPATH= cd -- "$(dirname "$rp")" 2>/dev/null && pwd)/$(basename "$rp")
    if [ "$rp" = "$AG_ROOT/ag" ]; then
      echo "ok    ag on PATH resolves to this install"
    else
      echo "FAIL  ag on PATH resolves to $p, not $AG_ROOT/ag — fix: another ag is shadowing this install (e.g. the Silver Searcher's \`ag\`); put $dir earlier in PATH, or give that other tool a different alias"
      bad=1
    fi
  fi
  for name in $(ag_config_keys alias); do
    case $name in
      '' | *[!A-Za-z0-9._-]*)
        echo "FAIL  alias '$name' is not a valid name — fix: rename it in [alias] in $(ag_config_file)"
        bad=1
        continue ;;
    esac
    f=$dir/$name
    if [ ! -x "$f" ] || ! grep -q "^$AG_SHIM_MARK" "$f" 2>/dev/null; then
      echo "FAIL  alias $name has no shim — fix: ag shims"
      bad=1
      continue
    fi
    line=$(grep '^exec ' "$f" 2>/dev/null | head -n 1)
    ag_path=""
    case $line in
      *"'"*"'"*)
        ag_path=${line#*\'}
        ag_path=${ag_path%%\'*} ;;
    esac
    if [ -n "$ag_path" ] && [ ! -x "$ag_path" ]; then
      echo "FAIL  alias $name points at a missing ag ($ag_path) — fix: ag shims"
      bad=1
      continue
    fi
    echo "ok    alias $name"
  done
  return $bad
}

ag_cmd_install() {
  [ $# -eq 1 ] || ag_die 2 "usage: ag install <harness>"
  ag_is_harness "$1" || ag_die 2 "unknown harness '$1' (known: $(ag_harnesses | tr '\n' ' '))"
  (ag_load_driver "$1" && drv_install_hint)
}
