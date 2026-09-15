#!/usr/bin/env bash
# Regenerate the tmux attach fixture used by src/lib/ansi.tmux-fixture.test.ts:
# what an 80x24 client receives when it attaches to a pane, followed by
# column-addressed updates, plus tmux's own capture-pane of the result.
# Linux only (util-linux `script`). Needs tmux; widths reflect THIS tmux version
# (the committed recording is tmux 3.6a). Run from the repository root.
set -euo pipefail
out=${1:-src/lib/__fixtures__}
mkdir -p "$out"
sock="cf-fixture-$$"
work=$(mktemp -d)
trap 'tmux -L "$sock" kill-server 2>/dev/null || true; rm -rf "$work"' EXIT

cat > "$work/content.sh" <<'EOS'
printf '\e[38;2;255;120;0m⏺ truecolor\e[0m \e[1;4mbold-ul\e[0m \e[7mrev\e[0m\n'
printf '╭──────╮ ⎿ ✻ │ █\n'
printf 'CJK:中文X\n'
printf 'emoji:\xf0\x9f\xa4\x96X\n'
printf 'accent:a\xcc\x81X\n'
printf 'vs16:X\xef\xb8\x8fX\n'
# ZWJ family (man ZWJ woman ZWJ girl): one 2-cell cluster in tmux 3.6a.
printf 'zwj:\xf0\x9f\x91\xa8\xe2\x80\x8d\xf0\x9f\x91\xa9\xe2\x80\x8d\xf0\x9f\x91\xa7X\n'
# Waving hand + skin tone U+1F3FD: the modifier joins its base.
printf 'skin:\xf0\x9f\x91\x8b\xf0\x9f\x8f\xbdX\n'
printf 'long:%s\n' "$(printf 'abcdefghij%.0s' $(seq 1 12))"
# Block until a client is attached, so the updates below reach it as
# incremental, column-addressed redraws rather than part of the attach paint.
tmux -L "$SOCK" wait-for attached
printf '\e[3;9HY\e[4;8HZ\e[5;9HY\e[6;8HQ\e[7;7HQ\e[8;8HQ\e[13;1H@@done@@\n'
tmux -L "$SOCK" wait-for -S done
exec cat
EOS

tmux -L "$sock" -f /dev/null new-session -d -s fx -x 80 -y 24 "SOCK=$sock bash $work/content.sh"
tmux -L "$sock" set-hook -g client-attached "run-shell 'tmux -L $sock wait-for -S attached'"
# A FIFO opened read-write never reaches EOF, so `script` does not forward an
# end-of-input byte into the pane.
mkfifo "$work/stdin"
exec 3<>"$work/stdin"
script -qfc "stty rows 24 cols 80; TERM=xterm-256color tmux -L $sock attach -t fx" "$work/raw" <&3 >/dev/null 2>&1 &
spid=$!
timeout 10 tmux -L "$sock" wait-for done
tmux -L "$sock" capture-pane -p -t fx > "$out/tmux-attach.pane.txt"
tmux -L "$sock" detach-client -s fx
wait "$spid" || true
if ! grep -aq '@@done@@' "$work/raw"; then
  echo "capture missed the final update (tmux flush race); rerun" >&2
  exit 1
fi
# Drop util-linux's "Script started …" header (a timestamp and this run's
# socket name) so a regeneration only diffs where tmux's output changed.
sed '1{/^Script started/d}' "$work/raw" > "$out/tmux-attach.raw.txt"
echo "wrote $out/tmux-attach.{raw,pane}.txt with $(tmux -V)"
