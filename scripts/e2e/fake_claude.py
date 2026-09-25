#!/usr/bin/env python3
"""A scripted stand-in for Claude Code's REPL, for scripts/hub-e2e.sh.

Work graph M10.2. The hub launches a session's pane as
`cl --resume <uuid> --name <tmux> || cl --session-id <uuid> ...`, calling a
`cl` on its PATH before falling back to `claude` (tmux.rs). The e2e copies
this file there as `cl`. It never talks to a model and never posts hooks:
the e2e posts the hooks itself, so every step is deterministic.

What it does:

* prints the cue fleet reads as "the REPL is ready" (`? for shortcuts`,
  `pane_shows_repl` in service/tasks.rs), so a start prompt gets typed;
* echoes what is typed (the tty does, in cooked mode);
* when a pasted prompt asks for a safe remove (SAFE_REMOVE_READY_<nonce>),
  answers on its own line with that marker once the paste has settled, so
  the marker is the LAST one in the pane (safe_kill.rs reads the last);
* when a pasted prompt asks for a handover (WORK_HANDOVER_BEGIN_<nonce>),
  prints a note between the markers (for a person watching; the hub reads
  the Stop hook the e2e posts, not the pane).
"""

import re
import select
import sys

CUE = "? for shortcuts"
SETTLE_SECS = 0.8


def say(line=""):
    sys.stdout.write(line + "\n")
    sys.stdout.flush()


def answer(text):
    ready = re.search(r"SAFE_REMOVE_READY_([0-9a-fA-F]+)", text)
    if ready:
        say("Checked the worktree; nothing left to save.")
        say(f"SAFE_REMOVE_READY_{ready.group(1)}")
    handover = re.search(r"WORK_HANDOVER_BEGIN_([0-9a-fA-F]+)", text)
    if handover:
        n = handover.group(1)
        say(f"WORK_HANDOVER_BEGIN_{n}")
        say("Done: the fake did its part.")
        say(f"WORK_HANDOVER_END_{n}")


def main():
    say("fake claude for claude-fleet e2e: " + " ".join(sys.argv[1:]))
    say(CUE)
    pending = []
    while True:
        ready, _, _ = select.select([sys.stdin], [], [], SETTLE_SECS)
        if ready:
            line = sys.stdin.readline()
            if not line:
                return
            pending.append(line)
            continue
        if pending:
            answer("".join(pending))
            pending = []
            say(CUE)


if __name__ == "__main__":
    main()
