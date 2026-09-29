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
