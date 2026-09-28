#!/usr/bin/env bash
# release-verify-sig.sh <pubkeys-file> <file> <file.minisig> [manifest|channel] [track]
# — exit 0 when a trusted key verifies the signature.
#
# With FLEET_RELEASE set and a kind given, the check is the fleet's own
# (`fleet-release verify`, i.e. fleet_update::verify: prehashed only, schema,
# track): what CI publishes is proven readable by the code that reads it.
# Otherwise `minisign -V -H` (prehashed required) against each key.
set -euo pipefail
keys="${1:?usage: release-verify-sig.sh <pubkeys-file> <file> <file.minisig> [kind] [track]}"
file="${2:?}"
sig="${3:?}"
kind="${4:-}"
track="${5:-}"
if [ -n "${FLEET_RELEASE:-}" ] && [ -n "$kind" ]; then
  args=(verify --keys-file "$keys" --kind "$kind" --file "$file" --sig "$sig")
  [ -n "$track" ] && args+=(--track "$track")
  if "$FLEET_RELEASE" "${args[@]}"; then exit 0; fi
else
  while IFS= read -r k; do
    [ -n "$k" ] || continue
    if minisign -V -H -q -P "$k" -m "$file" -x "$sig" >/dev/null 2>&1; then
      exit 0
    fi
  done <"$keys"
fi
echo "::error title=signature not trusted::$(basename "$file") is not signed by any key in crates/fleet-update/src/keys.rs — the signing secret and the compiled-in keys disagree."
exit 1
