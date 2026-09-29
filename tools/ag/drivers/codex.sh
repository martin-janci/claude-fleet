# shellcheck shell=bash
# Driver: OpenAI Codex CLI (`codex`). https://github.com/openai/codex
#
# On a Mac with only the ChatGPT / Codex desktop app, the CLI ships inside
# the app bundle and is not on PATH; AG_CODEX_FALLBACKS lists where to look
# (space-separated; set it to "" to disable).

drv_bin() {
  local p
  if p=$(command -v codex 2>/dev/null); then
    printf '%s\n' "$p"
    return 0
  fi
  for p in ${AG_CODEX_FALLBACKS-/Applications/ChatGPT.app/Contents/Resources/codex /Applications/Codex.app/Contents/Resources/codex}; do
    if [ -x "$p" ]; then
      printf '%s\n' "$p"
      return 0
    fi
  done
  return 1
}

drv_install_hint() {
  echo 'npm install -g @openai/codex   # or: brew install --cask codex'
}

# drv_argv — fill ARGV from the AG_* globals:
#   codex [exec] [resume --last | resume [ID]] [flags…] [passthrough…] [PROMPT]
# Codex cannot start a session under a caller-chosen id: --new-id → exit 3.
# --name has no Codex equivalent and is dropped. AG_RESUME_PICKER=1 (bare
# -r/--resume) maps to a bare `resume` — Codex's own resume picker; combined
# with -p that is `exec resume` with no id, which Codex cannot run
# non-interactively, so that combination is a usage error (exit 2).
drv_argv() {
  if [ -n "$AG_NEW_ID" ]; then
    echo "ag: codex cannot start a session under a chosen id (--new-id)" >&2
    return 3
  fi
  if [ "$AG_HAS_PRINT" = 1 ] && [ "$AG_RESUME_PICKER" = 1 ]; then
    echo "ag: codex needs a session id to resume non-interactively — pass -r ID, or drop -p to use the picker" >&2
    return 2
  fi
  ARGV=("$(drv_bin)")
  if [ "$AG_HAS_PRINT" = 1 ]; then ARGV+=(exec); fi
  if [ "$AG_CONTINUE" = 1 ]; then
    ARGV+=(resume --last)
  elif [ "$AG_RESUME_PICKER" = 1 ]; then
    ARGV+=(resume)
  elif [ -n "$AG_RESUME" ]; then
    ARGV+=(resume "$AG_RESUME")
  fi
  if [ "$AG_YOLO" = 1 ]; then ARGV+=(--dangerously-bypass-approvals-and-sandbox); fi
  if [ -n "$AG_MODEL" ]; then ARGV+=(-m "$AG_MODEL"); fi
  if [ -n "$AG_EFFORT" ]; then ARGV+=(-c "model_reasoning_effort=\"$AG_EFFORT\""); fi
  ARGV+=(${AG_PASS[@]+"${AG_PASS[@]}"})
  if [ "$AG_HAS_PRINT" = 1 ]; then ARGV+=("$AG_PRINT"); fi
  return 0
}
