# shellcheck shell=bash
# shellcheck disable=SC2034  # the AG_* globals are read by lib/launch.sh and the drivers
# ag's normalised flags. ag_parse_args fills these globals; drivers read them.
#
# Value-taking flags (-p/--print, -r/--resume, --new-id, --name, -m/--model,
# --effort) also accept `--flag=value`. A space-separated value that starts
# with "-" is rejected (exit 2) — it is almost always a forgotten `--`, not a
# real value; pass the harness's own same-letter flags after `--` instead
# (e.g. `cl -- -c key=val`). The one exception is -r/--resume: with no
# following value, or a following value that starts with "-", it means "open
# the harness's own resume picker" — AG_RESUME_PICKER=1, AG_RESUME stays "".

AG_H=""         # harness named as the first argument, if any
AG_HAS_PRINT=0  # 1 when -p/--print was given (the prompt itself may be empty)
AG_PRINT=""
AG_CONTINUE=0
AG_RESUME=""
AG_RESUME_PICKER=0  # 1 when -r/--resume was given with no (usable) id
AG_NEW_ID=""
AG_NAME=""
AG_MODEL=""
AG_EFFORT=""
AG_YOLO=""      # "" = take `yolo` from the config; 1 / 0 = explicit flag
AG_PASS=()      # handed to the harness unchanged

# ag_opt_value FLAG VALUE — reject a value that looks like another flag: the
# caller almost certainly forgot to quote it, or meant a flag of the
# harness's own (use -- for that).
ag_opt_value() {
  case $2 in
    -*) ag_die 2 "$1 takes a value; to pass the CLI's own flags use --  (e.g. cl -- -c key=val)" ;;
  esac
}

ag_parse_args() {
  if [ $# -gt 0 ] && ag_is_harness "$1"; then
    AG_H=$1
    shift
  fi
  while [ $# -gt 0 ]; do
    case $1 in
      -p | --print)
        [ $# -ge 2 ] || ag_die 2 "$1 needs a prompt"
        ag_opt_value "$1" "$2"
        AG_HAS_PRINT=1; AG_PRINT=$2; shift 2 ;;
      --print=*) AG_HAS_PRINT=1; AG_PRINT=${1#--print=}; shift ;;
      -c | --continue) AG_CONTINUE=1; shift ;;
      -r | --resume)
        if [ $# -ge 2 ]; then
          case $2 in
            -*) AG_RESUME_PICKER=1; shift ;;
            *) AG_RESUME=$2; shift 2 ;;
          esac
        else
          AG_RESUME_PICKER=1; shift
        fi ;;
      --resume=*) AG_RESUME=${1#--resume=}; shift ;;
      --new-id)
        [ $# -ge 2 ] || ag_die 2 "$1 needs a session id"
        ag_opt_value "$1" "$2"
        AG_NEW_ID=$2; shift 2 ;;
      --new-id=*) AG_NEW_ID=${1#--new-id=}; shift ;;
      --name)
        [ $# -ge 2 ] || ag_die 2 "$1 needs a name"
        ag_opt_value "$1" "$2"
        AG_NAME=$2; shift 2 ;;
      --name=*) AG_NAME=${1#--name=}; shift ;;
      -m | --model)
        [ $# -ge 2 ] || ag_die 2 "$1 needs a model"
        ag_opt_value "$1" "$2"
        AG_MODEL=$2; shift 2 ;;
      --model=*) AG_MODEL=${1#--model=}; shift ;;
      --effort)
        [ $# -ge 2 ] || ag_die 2 "$1 needs a level"
        ag_opt_value "$1" "$2"
        AG_EFFORT=$2; shift 2 ;;
      --effort=*) AG_EFFORT=${1#--effort=}; shift ;;
      --yolo) AG_YOLO=1; shift ;;
      --no-yolo) AG_YOLO=0; shift ;;
      --)
        shift
        while [ $# -gt 0 ]; do AG_PASS+=("$1"); shift; done ;;
      *) AG_PASS+=("$1"); shift ;;
    esac
  done
  case $AG_EFFORT in
    '') ;;
    *[!A-Za-z0-9_-]*) ag_die 2 "--effort: invalid value '$AG_EFFORT' (expected only letters, digits, _ or -)" ;;
  esac
  if [ "$AG_CONTINUE" = 1 ] && { [ -n "$AG_RESUME" ] || [ "$AG_RESUME_PICKER" = 1 ]; }; then
    ag_die 2 "--continue and --resume are mutually exclusive"
  fi
  if [ -n "$AG_NEW_ID" ] && { [ "$AG_CONTINUE" = 1 ] || [ -n "$AG_RESUME" ] || [ "$AG_RESUME_PICKER" = 1 ]; }; then
    ag_die 2 "--new-id starts a new session; drop --continue / --resume"
  fi
}
