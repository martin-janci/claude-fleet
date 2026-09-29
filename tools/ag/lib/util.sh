# shellcheck shell=bash
# Helpers shared by every ag command.

# ag_die CODE MSG… — print "ag: MSG" to stderr and exit CODE.
# Codes: 2 usage, 3 flag unsupported by the harness, 4 harness not
# installed, 5 config / filesystem problem.
ag_die() {
  local code=$1
  shift
  echo "ag: $*" >&2
  exit "$code"
}

ag_usage() {
  cat <<'EOF'
ag — one launcher for coding-agent CLIs

Usage:
  ag [<harness>] [flags] [-- raw args…]   start an agent (default harness from the config)
  ag list                                  harnesses ag knows and where they are installed
  ag which <harness>                       path of a harness binary
  ag install <harness>                     print the official install command
  ag shims                                 (re)generate alias commands from [alias] in the config
  ag doctor                                check binaries, config, PATH and shims

Flags (mapped onto each harness's own):
  -p, --print PROMPT   run once, non-interactively, and print the answer
  -c, --continue       continue the most recent session
  -r, --resume ID      resume session ID
      --new-id ID      start a new session under ID (Claude only; exit 3 elsewhere)
      --name N         label the session (Claude only; ignored elsewhere)
  -m, --model M        model id or alias
      --effort E       reasoning effort (low | medium | high …)
      --yolo           skip permission prompts; --no-yolo overrides `yolo = true`
  --                   pass everything after it to the harness unchanged
Unknown arguments are passed to the harness unchanged too.

Config: $AG_CONFIG, else ~/.config/ag/config
EOF
}
