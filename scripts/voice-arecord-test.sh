#!/usr/bin/env bash
# Test for tools/voice/arecord against fake servers. bash + python3 only.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ARECORD="$ROOT/tools/voice/arecord"
TMP="$(mktemp -d)"
PIDS=()
cleanup() {
  for p in "${PIDS[@]:-}"; do
    if [ -n "$p" ]; then kill "$p" 2>/dev/null || true; fi
  done
  rm -rf "$TMP"
}
trap cleanup EXIT

fail() { echo "FAIL: $*" >&2; exit 1; }
free_port() {
  python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])'
}

export HOME="$TMP/home"
mkdir -p "$HOME/.claude" "$HOME/.claude-fleet/voice" "$TMP/bin"
PORT="$(free_port)"
FB_PORT="$(free_port)"
printf 'FLEET_VOICE_URL=http://127.0.0.1:%s\n' "$PORT" >"$HOME/.claude-fleet/voice/voice.env"
printf 'Authorization: Bearer good\n' >"$HOME/.claude/fleet-hook.headers"

# The recorder asks for the session of its own pane (`-t "$TMUX_PANE"`):
# pane %2 is in session t2, %3 in t3, any other pane (or none) in t1.
cat >"$TMP/bin/tmux" <<'T'
#!/usr/bin/env bash
[ "$1" = display-message ] && [ "$2" = -p ] || exit 1
if [ "$3" = -t ] && [ "$5" = '#S' ]; then
  case "$4" in %2) echo t2 ;; %3) echo t3 ;; *) echo t1 ;; esac; exit 0
fi
if [ "$3" = '#S' ]; then echo t1; exit 0; fi
exit 1
T
chmod +x "$TMP/bin/tmux"
export PATH="$TMP/bin:$PATH"
export TMUX=/tmp/x,1,0
unset TMUX_PANE
# Every temp file the recorder makes lands here, so a leftover is seen.
export TMPDIR="$TMP/tmpdir"
mkdir -p "$TMPDIR"
export FLEET_VOICE_FALLBACK_PORT="$FB_PORT"

cat >"$TMP/fake_http.py" <<'P'
import os, sys, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
MARK = sys.argv[2]
class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        ok = self.path.split("?")[0] == "/voice/capture" \
             and self.headers.get("Authorization") == "Bearer good"
        if ok and "tmux=t2" in self.path:
            # A live microphone: streams until the recorder hangs up, and
            # says when it did.
            self.send_response(200); self.end_headers()
            open(os.path.join(MARK, "t2.connected"), "w").close()
            try:
                while True:
                    self.wfile.write(b"\x07" * 3200); self.wfile.flush()
                    time.sleep(0.05)
            except OSError:
                open(os.path.join(MARK, "t2.gone"), "w").close()
            return
        if ok and "tmux=t3" in self.path:
            # The connection drops mid-recording.
            self.send_response(200); self.send_header("Content-Length", "32000")
            self.end_headers(); self.wfile.write(b"\x07" * 3200); self.wfile.flush()
            self.close_connection = True
            return
        if ok and "tmux=t1" in self.path:
            self.send_response(200)
            self.send_header("Content-Length", "32000")
            self.end_headers()
            self.wfile.write(b"\x07" * 32000)
        else:
            self.send_response(409); self.send_header("Content-Length", "0"); self.end_headers()
ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
P
cat >"$TMP/fake_fb.py" <<'P'
import socket, sys
s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", int(sys.argv[1]))); s.listen(5)
while True:
    c, _ = s.accept()
    line = b""
    while not line.endswith(b"\n"):
        d = c.recv(1)
        if not d: break
        line += d
    try: want = open(sys.argv[2]).read().strip()
    except OSError: want = None
    if want is not None and line.decode().strip() == want:
        c.sendall(b"\x07" * 3200)
    c.close()
P

python3 "$TMP/fake_http.py" "$PORT" "$TMP" & PIDS+=($!)
mkdir -p "$HOME/.config/fleet-voice"  # token file is written only by assertion 5
python3 "$TMP/fake_fb.py" "$FB_PORT" "$HOME/.config/fleet-voice/token" & PIDS+=($!)
for _ in $(seq 50); do
  python3 -c "import socket,sys
for p in ($PORT,$FB_PORT): socket.create_connection(('127.0.0.1',p),1).close()" 2>/dev/null && break
  sleep 0.1
done

# 1. --version
"$ARECORD" --version >/dev/null || fail "1: --version"
echo "ok 1 --version"

# 2. probe within 2 s
timeout 2 "$ARECORD" -f S16_LE -r 16000 -c 1 -t raw /dev/null || fail "2: probe"
echo "ok 2 probe"

# 3. record streams all PCM
n="$("$ARECORD" -f S16_LE -r 16000 -c 1 -t raw -q - | wc -c)"
[ "$n" -eq 32000 ] || fail "3: got $n bytes, want 32000"
echo "ok 3 record 32000 bytes"

# 4. bad bearer, no fallback token: exit 1 with hint
printf 'Authorization: Bearer bad\n' >"$HOME/.claude/fleet-hook.headers"
FB_TOKEN="$HOME/.config/fleet-voice/token"
set +e
"$ARECORD" -f S16_LE -r 16000 -c 1 -t raw -q - >"$TMP/out" 2>"$TMP/err"
rc=$?
set -e
[ "$rc" -eq 1 ] || fail "4: exit $rc, want 1"
grep -q 'turn on 🎤' "$TMP/err" || fail "4: stderr lacks hint: $(cat "$TMP/err")"
[ ! -s "$TMP/out" ] || fail "4: stdout not empty"
echo "ok 4 hint on 409"

# 5. fallback relay with token
printf 'sekrit\n' >"$FB_TOKEN"
n="$("$ARECORD" -f S16_LE -r 16000 -c 1 -t raw -q - 2>/dev/null | wc -c)"
[ "$n" -eq 3200 ] || fail "5: got $n bytes, want 3200"
echo "ok 5 fallback 3200 bytes"

# 6. -l unsupported
set +e; "$ARECORD" -l >/dev/null 2>&1; rc=$?; set -e
[ "$rc" -eq 2 ] || fail "6: exit $rc, want 2"
echo "ok 6 -l exits 2"

# Poll `cond` (a command) for up to 5 s.
within5() { for _ in $(seq 50); do "$@" && return 0; sleep 0.1; done; return 1; }
alive() { kill -0 "$1" 2>/dev/null; }
not_alive() { ! alive "$1"; }

# 7. Claude Code ends a recording with SIGTERM to arecord and waits for its
#    stdout to close: the stream must end at once, the server see the
#    hang-up, and no temp file stay behind. No fallback may answer instead.
printf 'Authorization: Bearer good\n' >"$HOME/.claude/fleet-hook.headers"
rm -f "$FB_TOKEN"
mkfifo "$TMP/pipe"
cat "$TMP/pipe" >"$TMP/rec" & reader=$!; PIDS+=("$reader")
TMUX_PANE=%2 "$ARECORD" -f S16_LE -r 16000 -c 1 -t raw -q - >"$TMP/pipe" 2>"$TMP/err7" &
rec=$!; PIDS+=("$rec")
within5 test -e "$TMP/t2.connected" || fail "7: the recorder never connected as t2 (pane %2)"
within5 test -s "$TMP/rec" || fail "7: no audio reached the reader"
kill -TERM "$rec"
within5 not_alive "$reader" || fail "7: the reader got no EOF after SIGTERM (curl left streaming)"
within5 test -e "$TMP/t2.gone" || fail "7: the server never saw the recorder hang up"
left="$(ls -A "$TMPDIR")"
[ -z "$left" ] || fail "7: temp files left behind: $left"
echo "ok 7 SIGTERM ends the stream"

# 8. A stream cut mid-recording fails; it is not continued from the manual
#    relay (whose 3 200 bytes would follow the first 3 200).
printf 'sekrit\n' >"$FB_TOKEN"
set +e
TMUX_PANE=%3 "$ARECORD" -f S16_LE -r 16000 -c 1 -t raw -q - >"$TMP/out8" 2>"$TMP/err8"
rc=$?
set -e
n="$(wc -c <"$TMP/out8" | tr -d ' ')"
[ "$n" -eq 3200 ] || fail "8: got $n bytes, want the 3200 before the cut (fallback ran?)"
[ "$rc" -ne 0 ] || fail "8: exit 0 after a cut stream"
left="$(ls -A "$TMPDIR")"
[ -z "$left" ] || fail "8: temp files left behind: $left"
echo "ok 8 a cut stream is not continued from the fallback"
echo "all 8 passed"
