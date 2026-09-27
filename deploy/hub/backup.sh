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
# (PREFIX=pre-<version> KEEP=3). Retention is per PREFIX, by count.
# Exit codes: 0 backed up and verified; 1 the copy failed (removed); 2 bad input.
set -euo pipefail

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

mkdir -p "$OUT_DIR"
OUT="$OUT_DIR/$PREFIX-$(date -u +%Y%m%d-%H%M%S).db"
if ! "$SQLITE" "$DB" ".backup '$OUT'"; then
  echo "backup: .backup of $DB failed; removing $OUT" >&2
  rm -f -- "$OUT"
  exit 1
fi
if ! "$SQLITE" "$OUT" 'PRAGMA integrity_check' | grep -qx ok; then
  echo "backup: integrity_check failed on $OUT; removed" >&2
  rm -f -- "$OUT"
  exit 1
fi
# Retention: the newest $KEEP files of this prefix stay. The names carry no
# whitespace (this script wrote them), so word-splitting `ls -1t` is safe.
n=0
for f in $(ls -1t "$OUT_DIR"/"$PREFIX"-*.db 2>/dev/null); do
  n=$((n + 1))
  if [ "$n" -gt "$KEEP" ]; then rm -f -- "$f"; fi
done
if [ "$(id -u)" = 0 ]; then
  chown -R "$OWNER" "$OUT_DIR"
  chmod 750 "$OUT_DIR"
fi
echo "backup: ok $OUT ($(wc -c <"$OUT" | tr -d ' ') bytes; keeping the newest $KEEP $PREFIX-*.db)"
