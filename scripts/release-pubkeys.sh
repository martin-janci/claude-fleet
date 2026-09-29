#!/usr/bin/env bash
# release-pubkeys.sh — print the release public keys every build trusts, one
# per line, from `crates/fleet-update/src/keys.rs` (RELEASE_KEYS). The release
# workflows check every signature they make against these, so a document is
# never published signed by a key the fleet does not trust.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
# A minisign public key is 56 base64 characters starting with `RW` ("Ed").
# FLEET_RELEASE_KEYS_RS: another keys.rs (the scripts' own test).
grep -oE '"RW[A-Za-z0-9+/]{54}"' "${FLEET_RELEASE_KEYS_RS:-$root/crates/fleet-update/src/keys.rs}" | tr -d '"' || true
