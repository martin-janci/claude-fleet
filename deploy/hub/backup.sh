#!/usr/bin/env bash
# deploy/hub/backup.sh — a consistent, online backup of the hub's state.db.
#
# `sqlite3 .backup` uses SQLite's online backup API: one self-contained file
# with the WAL folded in, taken while the hub keeps writing. A `cp` of
# state.db from a running hub is NOT that — it misses every page still in
# state.db-wal (the NAS's backup-* dirs were exactly that: state.db minutes
# older than its -wal), and a `cp` of the triple can be torn mid-write.
#
# Run as root from a DSM Task Scheduler entry (nightly) and from upgrade.sh
# (PREFIX=pre-<version> KEEP=3). Retention is per PREFIX, by count: a run
# prunes only files of its own PREFIX, so pre-<older version> backups are
# never pruned by a later upgrade — delete those by hand.
# A copy holds everything state.db does, the master token included: it is
# written 0600 (umask 077) into a 0700 directory, root or not.
# Exit codes: 0 backed up and verified; 1 the copy failed (removed); 2 bad input.
set -euo pipefail
umask 077

DATA="${FLEET_HUB_DATA:-/volume1/docker/fleet-hub/data}"
OUT_DIR="${FLEET_HUB_BACKUPS:-$(dirname "$DATA")/backups}"
PREFIX="${PREFIX:-state}"
KEEP="${KEEP:-14}"
SQLITE="${SQLITE:-sqlite3}"
OWNER="${FLEET_HUB_OWNER:-1000}"
DB="$DATA/state.db"

command -v "$SQLITE" >/dev/null 2>&1 || { echo "backup: $SQLITE not found" >&2; exit 2; }
[ -f "$DB" ] || { echo "backup: no database at $DB" >&2; exit 2; }
case "$KEEP" in ''|*[!0-9]*) echo "backup: KEEP must be a number, got '$KEEP'" >&2; exit 2;; esac
case "$PREFIX" in *[!A-Za-z0-9._-]*|'') echo "backup: PREFIX must be [A-Za-z0-9._-]+, got '$PREFIX'" >&2; exit 2;; esac

case "$OUT_DIR" in *$'\n'*) echo "backup: FLEET_HUB_BACKUPS must not contain a newline" >&2; exit 2;; esac

mkdir -p "$OUT_DIR"
# Tightened before the copy exists, not after: a directory an older version
# of this script left 0755 (or a hand-made one) must not expose it.
chmod 700 "$OUT_DIR"
# The final name is never one that already exists (two runs in the same
# second — upgrade.sh right after a manual run — get `-1`, `-2`, …), and the
# copy is written to a `.part` file first, so a failed or corrupt copy is
# removed without ever touching an earlier good backup of the same second.
STAMP="$(date -u +%Y%m%d-%H%M%S)"
OUT="$OUT_DIR/$PREFIX-$STAMP.db"
n=0
while [ -e "$OUT" ]; do
  n=$((n + 1))
  OUT="$OUT_DIR/$PREFIX-$STAMP-$n.db"
done
PART="$OUT.part"
# A run killed mid-copy (SIGINT/SIGTERM/HUP) must not leave its `.part`
# behind either; after the `mv` below the name no longer exists. Only a
# SIGKILL can still strand one.
trap 'rm -f -- "$PART"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM HUP
# The dot-command's argument is double-quoted, where sqlite3 resolves
# backslash escapes (a single-quoted one has no escape at all, so a `'` in
# the path would end it): `\` and `"` are escaped, nothing else is special.
QPART="${PART//\\/\\\\}"
QPART="${QPART//\"/\\\"}"
if ! "$SQLITE" "$DB" ".backup \"$QPART\""; then
  echo "backup: .backup of $DB failed; removing $PART" >&2
  rm -f -- "$PART"
  exit 1
fi
if ! "$SQLITE" "$PART" 'PRAGMA integrity_check' | grep -qx ok; then
  echo "backup: integrity_check failed on $PART; removed" >&2
  rm -f -- "$PART"
  exit 1
fi
chmod 600 "$PART"
mv -- "$PART" "$OUT"
# Retention: the newest $KEEP files of this prefix stay. The names carry no
# whitespace (this script wrote them), so word-splitting `ls -1t` is safe.
# `$PREFIX-<digit>` because the stamp starts with a digit: PREFIX=pre-1.0.0
# must not prune pre-1.0.0-rc1-*.db.
n=0
# shellcheck disable=SC2045  # newest first needs ls -t; the names are ours
for f in $(ls -1t "$OUT_DIR"/"$PREFIX"-[0-9]*.db 2>/dev/null); do
  n=$((n + 1))
  if [ "$n" -gt "$KEEP" ]; then rm -f -- "$f"; fi
done
if [ "$(id -u)" = 0 ]; then
  chown -R "$OWNER" "$OUT_DIR"
fi
echo "backup: ok $OUT ($(wc -c <"$OUT" | tr -d ' ') bytes; keeping the newest $KEEP $PREFIX-*.db)"
