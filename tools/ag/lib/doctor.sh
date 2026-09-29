# shellcheck shell=bash
# `ag doctor`: is this machine ready to launch agents? One line per check;
# every FAIL ends with the exact fix. Exit 1 if anything failed.
# `ag install <h>`: print the harness's official install command.

ag_cmd_doctor() {
  local bad=0 h p v d dir name f
  for h in $(ag_harnesses); do
    if p=$(ag_bin "$h"); then
      v=$("$p" --version 2>/dev/null | head -n 1)
      echo "ok    $h: $p${v:+ ($v)}"
    else
      echo "--    $h: not installed (install: $(ag_load_driver "$h" && drv_install_hint))"
    fi
  done
  if d=$(ag_resolve_harness ""); then
    if ag_is_harness "$d" && ag_bin "$d" >/dev/null; then
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
  for name in $(ag_config_keys alias); do
    f=$dir/$name
    if [ -x "$f" ] && grep -q "^$AG_SHIM_MARK" "$f" 2>/dev/null; then
      echo "ok    alias $name"
    else
      echo "FAIL  alias $name has no shim — fix: ag shims"
      bad=1
    fi
  done
  return $bad
}

ag_cmd_install() {
  [ $# -eq 1 ] || ag_die 2 "usage: ag install <harness>"
  ag_is_harness "$1" || ag_die 2 "unknown harness '$1' (known: $(ag_harnesses | tr '\n' ' '))"
  (ag_load_driver "$1" && drv_install_hint)
}
