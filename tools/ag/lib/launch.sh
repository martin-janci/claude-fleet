# shellcheck shell=bash
# Resolve the harness, build its argv through the driver, exec it.

# ag_launch ARGS… — never returns on success (exec).
ag_launch() {
  local h
  ag_parse_args "$@"
  h=$(ag_resolve_harness "$AG_H") || ag_die 4 "no agent CLI found — see: ag list, ag install claude"
  ag_is_harness "$h" || ag_die 2 "unknown harness '$h' (known: $(ag_harnesses | tr '\n' ' '))"
  ag_load_driver "$h"
  if ! drv_bin >/dev/null; then
    echo "ag: $h is not installed. Install it with:" >&2
    echo "  $(drv_install_hint)" >&2
    exit 4
  fi
  if [ -z "$AG_YOLO" ]; then
    case $(ag_config_get "" yolo) in
      true | yes | 1) AG_YOLO=1 ;;
      *) AG_YOLO=0 ;;
    esac
  fi
  ARGV=()
  drv_argv || exit $?
  exec "${ARGV[@]}"
}
