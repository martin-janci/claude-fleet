#!/usr/bin/env bash
# Stands in for `ssh` in hub-e2e's install leg (CLAUDE_FLEET_SSH points the
# hub at it): runs the remote command on this machine, as a "host" whose HOME
# is $E2E_SSH_HOME, with its own tmux server under $E2E_SSH_TMUX. Control
# requests (`-O check`, `-O exit`) succeed and do nothing. Everything after
# `--` is the host and then the remote words, joined the way sshd hands them
# to the login shell. FLEET_AGENT_NO_SYSTEMD keeps the install job off
# systemd, so a CI runner never gets a user unit.
for a in "$@"; do [ "$a" = -O ] && exit 0; done
while [ $# -gt 0 ] && [ "$1" != -- ]; do shift; done
[ $# -ge 2 ] || { echo "e2e-fake-ssh: no host after --" >&2; exit 255; }
shift 2
exec env -u TMUX -u TMUX_PANE -u NOTIFY_SOCKET \
  HOME="${E2E_SSH_HOME:?}" XDG_CONFIG_HOME="$E2E_SSH_HOME/.config" \
  TMUX_TMPDIR="${E2E_SSH_TMUX:?}" FLEET_AGENT_NO_SYSTEMD=1 \
  bash -c "$*"
