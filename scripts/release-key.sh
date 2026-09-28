#!/usr/bin/env bash
# release-key.sh — create the release signing key, ONCE, on the owner's own
# machine (update-channel design §10, §13 question 1).
#
#   scripts/release-key.sh [--repo owner/repo] [--no-keychain]
#
# 1. generates a minisign key pair in a private temp dir (no password: CI
#    signs unattended, and the secret lives only in GitHub's encrypted
#    secrets and your keychain);
# 2. backs it up FIRST, in the macOS login keychain as
#    `claude-fleet-release-signing-key` (elsewhere, or with --no-keychain,
#    to ~/claude-fleet-release.key: move it somewhere safe);
# 3. stores the SECRET key as the repository secret RELEASE_SIGNING_KEY
#    (`gh secret set`, fed on stdin — never on a command line);
# 4. writes the PUBLIC key into crates/fleet-update/src/keys.rs and prints
#    it. Commit that change: every build from then on trusts it.
#
# The secret key never leaves this machine except into the GitHub secret.
# Refuses to run when keys.rs already names a key: a second key is a
# rotation (design §10), not a re-run.
set -euo pipefail

repo="martin-janci/claude-fleet"
keychain=1
while [ $# -gt 0 ]; do
  case "$1" in
    --repo) repo="${2:?--repo needs owner/repo}"; shift 2 ;;
    --no-keychain) keychain=0; shift ;;
    -h|--help) sed -n '2,/^set -euo/p' "$0" | sed 's/^# \{0,1\}//; $d'; exit 0 ;;
    *) echo "release-key.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done

root="$(cd "$(dirname "$0")/.." && pwd)"
# FLEET_RELEASE_KEYS_RS: another keys.rs (the scripts' own test).
keys_rs="${FLEET_RELEASE_KEYS_RS:-$root/crates/fleet-update/src/keys.rs}"
command -v minisign >/dev/null || { echo "release-key.sh: install minisign first (brew install minisign / apt install minisign)" >&2; exit 1; }
command -v gh >/dev/null || { echo "release-key.sh: install and log in to the GitHub CLI first (gh auth login)" >&2; exit 1; }
if [ -n "$("$root/scripts/release-pubkeys.sh")" ]; then
  echo "release-key.sh: $keys_rs already names a release key. A new one is a rotation: see docs/RELEASING.md." >&2
  exit 1
fi

umask 077
dir="$(mktemp -d)"
trap 'rm -f "$dir/release.key"; rmdir "$dir" 2>/dev/null || true' EXIT
minisign -G -W -f -p "$dir/release.pub" -s "$dir/release.key" >/dev/null
pub="$(sed -n 2p "$dir/release.pub")"
case "$pub" in RW*) ;; *) echo "release-key.sh: unexpected public key format" >&2; exit 1 ;; esac

if [ "$keychain" = 1 ] && command -v security >/dev/null; then
  security add-generic-password -U -a "$USER" -s claude-fleet-release-signing-key \
    -w "$(base64 <"$dir/release.key" | tr -d '\n')"
  echo "release-key.sh: backed the secret key up in the login keychain (claude-fleet-release-signing-key, base64)."
else
  if [ -e "$HOME/claude-fleet-release.key" ]; then
    echo "release-key.sh: $HOME/claude-fleet-release.key exists; move it away first (the GitHub secret is already set)." >&2
    exit 1
  fi
  cp "$dir/release.key" "$HOME/claude-fleet-release.key"
  echo "release-key.sh: NO keychain backup — the secret key is at ~/claude-fleet-release.key; move it somewhere safe now."
fi

# Only once the key is backed up: a secret set without a copy is a key lost.
gh secret set RELEASE_SIGNING_KEY --repo "$repo" <"$dir/release.key"
echo "release-key.sh: stored the secret key as $repo's RELEASE_SIGNING_KEY."

# The public key into keys.rs: the one line CI and every build read.
tmp="$(mktemp)"
awk -v k="$pub" '
  /^pub const RELEASE_KEYS: &\[&str\] = &\[\];$/ {
    print "pub const RELEASE_KEYS: &[&str] = &["
    print "    // Created by scripts/release-key.sh."
    print "    \"" k "\","
    print "];"
    done = 1
    next
  }
  { print }
  END { if (!done) exit 3 }
' "$keys_rs" >"$tmp" || { rm -f "$tmp"; echo "release-key.sh: could not find the empty RELEASE_KEYS in $keys_rs; add \"$pub\" by hand." >&2; exit 1; }
mv "$tmp" "$keys_rs"
chmod 644 "$keys_rs"

cat <<EOF

Public key (safe to share): $pub

Next: commit crates/fleet-update/src/keys.rs ("chore(update): trust the release key")
and merge it before the next release; release.yml signs from v0.4.1 on.
EOF
