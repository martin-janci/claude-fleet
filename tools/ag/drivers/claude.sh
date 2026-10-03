# shellcheck shell=bash
# Driver: Claude Code (`claude`). https://code.claude.com/docs

# `command -v` alone is not enough: in bash it returns a PATH entry that is
# not EXECUTABLE (verified: rc 0 and the path printed for a chmod -x file), so
# a half-finished install — an interrupted download, a file restored without
# its mode — reported the harness as present and then failed at launch with a
# permission error nothing had predicted.
drv_bin() {
  local p
  p=$(command -v claude 2>/dev/null) && [ -x "$p" ] && printf '%s\n' "$p"
}

drv_install_hint() {
  echo 'curl -fsSL https://claude.ai/install.sh | bash'
}

# drv_argv — fill ARGV from the AG_* globals, in the canonical order fleet's
# pane command and the tests rely on (see the plan's Global Constraints):
#   claude [--dangerously-skip-permissions] [--model M] [--effort E] [--name N]
#          [--continue|--resume [ID]|--session-id ID] [passthrough…] [-p PROMPT]
# AG_RESUME_PICKER=1 (bare -r/--resume, or one followed by another flag) maps
# to a bare `--resume` — Claude's own interactive resume picker.
drv_argv() {
  ARGV=("$(drv_bin)")
  if [ "$AG_YOLO" = 1 ]; then ARGV+=(--dangerously-skip-permissions); fi
  if [ -n "$AG_MODEL" ]; then ARGV+=(--model "$AG_MODEL"); fi
  if [ -n "$AG_EFFORT" ]; then ARGV+=(--effort "$AG_EFFORT"); fi
  if [ -n "$AG_NAME" ]; then ARGV+=(--name "$AG_NAME"); fi
  if [ "$AG_CONTINUE" = 1 ]; then
    ARGV+=(--continue)
  elif [ "$AG_RESUME_PICKER" = 1 ]; then
    ARGV+=(--resume)
  elif [ -n "$AG_RESUME" ]; then
    ARGV+=(--resume "$AG_RESUME")
  fi
  if [ -n "$AG_NEW_ID" ]; then ARGV+=(--session-id "$AG_NEW_ID"); fi
  ARGV+=(${AG_PASS[@]+"${AG_PASS[@]}"})
  if [ "$AG_HAS_PRINT" = 1 ]; then ARGV+=(-p "$AG_PRINT"); fi
  return 0
}
