# shellcheck shell=bash
# The harness registry: one driver file per harness in $AG_ROOT/drivers.

# ag_harnesses — every harness with a driver, one per line.
ag_harnesses() {
  local f
  for f in "$AG_ROOT"/drivers/*.sh; do
    [ -e "$f" ] || continue
    basename "$f" .sh
  done
}

# ag_is_harness NAME — true when NAME has a driver. Exact match against each
# driver id in turn — not a substring/case-pattern test — so a value with
# embedded spaces (e.g. "claude codex", two valid ids joined) never matches,
# and characters in NAME are never interpreted as glob wildcards.
ag_is_harness() {
  local h
  for h in $(ag_harnesses); do
    [ "$h" = "$1" ] && return 0
  done
  return 1
}

# ag_is_harness_ci NAME — case-insensitive ag_is_harness, for contexts (alias
# names) where "Claude" shadowing "claude" is just as unsafe as an exact hit.
ag_is_harness_ci() {
  local lc
  lc=$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')
  ag_is_harness "$lc"
}

# ag_load_driver H — source drivers/H.sh into this shell (drv_bin,
# drv_install_hint, drv_argv). Clears the previous driver's functions first.
ag_load_driver() {
  unset -f drv_bin drv_install_hint drv_argv 2>/dev/null
  # shellcheck source=/dev/null
  . "$AG_ROOT/drivers/$1.sh"
}

# ag_bin H — print H's binary path; exit 1 if it is not installed.
ag_bin() {
  (ag_load_driver "$1" && drv_bin)
}

# ag_resolve_harness [EXPLICIT] — the harness to use: EXPLICIT, else
# $AG_HARNESS, else config `default`, else the first installed one in config
# `order` (driver order when unset). Exit 1 when nothing qualifies.
ag_resolve_harness() {
  local d order h
  if [ -n "${1:-}" ]; then printf '%s\n' "$1"; return 0; fi
  if [ -n "${AG_HARNESS:-}" ]; then printf '%s\n' "$AG_HARNESS"; return 0; fi
  if d=$(ag_config_get "" default) && [ -n "$d" ]; then printf '%s\n' "$d"; return 0; fi
  order=$(ag_config_get "" order) || order=$(ag_harnesses | tr '\n' ' ')
  for h in $order; do
    if ag_is_harness "$h" && ag_bin "$h" >/dev/null 2>&1; then
      printf '%s\n' "$h"
      return 0
    fi
  done
  return 1
}

ag_cmd_list() {
  local h p
  for h in $(ag_harnesses); do
    p=$(ag_bin "$h") || p=-
    printf '%s %s\n' "$h" "$p"
  done
}

ag_cmd_which() {
  [ $# -eq 1 ] || ag_die 2 "usage: ag which <harness>"
  ag_is_harness "$1" || ag_die 2 "unknown harness '$1' (known: $(ag_harnesses | tr '\n' ' '))"
  ag_bin "$1" || ag_die 4 "$1 is not installed — see: ag install $1"
}
