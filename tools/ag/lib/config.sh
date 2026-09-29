# shellcheck shell=bash
# The INI-style config (~/.config/ag/config). Top-level keys: default, order,
# yolo.
#
#   default = claude   # which harness `ag` launches with no argument
#   order = claude codex
#   yolo = false
#   [alias]
#   cl = claude --yolo
#
# `#` / `;` start a comment line; a trailing `  #`/`  ;` after a value is
# stripped too (`key = value  # note`, the value must be followed by
# whitespace before the `#`/`;` or it stays part of the value); keys and
# values are trimmed; no quoting.

# ag_config_file — the config path ag reads.
ag_config_file() {
  printf '%s\n' "${AG_CONFIG:-${XDG_CONFIG_HOME:-$HOME/.config}/ag/config}"
}

# ag_config_get SECTION KEY — print KEY's value in [SECTION] ("" = top level).
# Exit 1 when the file or the key is missing.
ag_config_get() {
  local f
  f=$(ag_config_file)
  [ -r "$f" ] || return 1
  awk -v want_sec="$1" -v want_key="$2" '
    /^[[:space:]]*[#;]/ || /^[[:space:]]*$/ { next }
    /^[[:space:]]*\[/ { s = $0; gsub(/^[[:space:]]*\[|\][[:space:]]*$/, "", s); sec = s; next }
    {
      i = index($0, "="); if (i == 0) next
      k = substr($0, 1, i - 1); v = substr($0, i + 1)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", k)
      sub(/[[:space:]]+[#;].*$/, "", v)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", v)
      if (sec == want_sec && k == want_key) { print v; found = 1; exit }
    }
    END { exit found ? 0 : 1 }' "$f"
}

# ag_config_keys SECTION — every key in [SECTION], one per line, in file order.
ag_config_keys() {
  local f
  f=$(ag_config_file)
  [ -r "$f" ] || return 0
  awk -v want_sec="$1" '
    /^[[:space:]]*[#;]/ || /^[[:space:]]*$/ { next }
    /^[[:space:]]*\[/ { s = $0; gsub(/^[[:space:]]*\[|\][[:space:]]*$/, "", s); sec = s; next }
    {
      i = index($0, "="); if (i == 0) next
      k = substr($0, 1, i - 1); gsub(/^[[:space:]]+|[[:space:]]+$/, "", k)
      if (sec == want_sec) print k
    }' "$f"
}
