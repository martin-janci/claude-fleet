#!/bin/bash
# Web-session setup: the system packages this repo's scripts need that the
# base image lacks. Claude Code on the web only; a local session is the
# developer's own machine.
#
# openssh-client: `ssh-keygen`, which `fleet-hub ssh-key` runs and the
# ssh-key checks in scripts/hub-e2e.sh exercise.
set -euo pipefail

if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ]; then
  exit 0
fi

command -v ssh-keygen >/dev/null 2>&1 && exit 0

SUDO=""
[ "$(id -u)" -ne 0 ] && SUDO="sudo"
export DEBIAN_FRONTEND=noninteractive
$SUDO apt-get install -y -q openssh-client >/dev/null 2>&1 || {
  $SUDO apt-get update -q >/dev/null
  $SUDO apt-get install -y -q openssh-client >/dev/null
}
command -v ssh-keygen >/dev/null
