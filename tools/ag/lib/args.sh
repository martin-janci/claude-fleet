# shellcheck shell=bash
# shellcheck disable=SC2034  # the AG_* globals are read by lib/launch.sh and the drivers
# ag's normalised flags. ag_parse_args fills these globals; drivers read them.

AG_H=""         # harness named as the first argument, if any
AG_HAS_PRINT=0  # 1 when -p/--print was given (the prompt itself may be empty)
AG_PRINT=""
AG_CONTINUE=0
AG_RESUME=""
AG_NEW_ID=""
AG_NAME=""
AG_MODEL=""
AG_EFFORT=""
AG_YOLO=""      # "" = take `yolo` from the config; 1 / 0 = explicit flag
AG_PASS=()      # handed to the harness unchanged

ag_parse_args() {
  if [ $# -gt 0 ] && ag_is_harness "$1"; then
    AG_H=$1
    shift
  fi
  while [ $# -gt 0 ]; do
    case $1 in
      -p | --print)
        [ $# -ge 2 ] || ag_die 2 "$1 needs a prompt"
        AG_HAS_PRINT=1; AG_PRINT=$2; shift 2 ;;
      -c | --continue) AG_CONTINUE=1; shift ;;
      -r | --resume)
        [ $# -ge 2 ] || ag_die 2 "$1 needs a session id"
        AG_RESUME=$2; shift 2 ;;
      --new-id)
        [ $# -ge 2 ] || ag_die 2 "$1 needs a session id"
        AG_NEW_ID=$2; shift 2 ;;
      --name)
        [ $# -ge 2 ] || ag_die 2 "$1 needs a name"
        AG_NAME=$2; shift 2 ;;
      -m | --model)
        [ $# -ge 2 ] || ag_die 2 "$1 needs a model"
        AG_MODEL=$2; shift 2 ;;
      --effort)
        [ $# -ge 2 ] || ag_die 2 "$1 needs a level"
        AG_EFFORT=$2; shift 2 ;;
      --yolo) AG_YOLO=1; shift ;;
      --no-yolo) AG_YOLO=0; shift ;;
      --)
        shift
        while [ $# -gt 0 ]; do AG_PASS+=("$1"); shift; done ;;
      *) AG_PASS+=("$1"); shift ;;
    esac
  done
  if [ "$AG_CONTINUE" = 1 ] && [ -n "$AG_RESUME" ]; then
    ag_die 2 "--continue and --resume are mutually exclusive"
  fi
  if [ -n "$AG_NEW_ID" ] && { [ "$AG_CONTINUE" = 1 ] || [ -n "$AG_RESUME" ]; }; then
    ag_die 2 "--new-id starts a new session; drop --continue / --resume"
  fi
}
