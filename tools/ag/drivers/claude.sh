# shellcheck shell=bash
# Driver: Claude Code (`claude`). https://code.claude.com/docs

drv_bin() {
  command -v claude 2>/dev/null
}

drv_install_hint() {
  echo 'curl -fsSL https://claude.ai/install.sh | bash'
}
