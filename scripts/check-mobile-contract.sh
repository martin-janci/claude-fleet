#!/usr/bin/env bash
# Does fleet-mobile accept the hub wire-contract revision claude-fleet ships?
# Usage: scripts/check-mobile-contract.sh [--hub-ref <ref>] [--mobile-ref <ref>] [--print-hub-rev]
#   --hub-ref <ref>     read CONTRACT_REVISION from claude-fleet on GitHub at
#                       <ref> (default: this checkout's wire_contract.rs)
#   --mobile-ref <ref>  read fleet-mobile's MIN/MAX_HUB_CONTRACT at <ref>
#                       (default: main)
#   --print-hub-rev     print the hub revision and exit
# Env:   FLEET_MOBILE_REPO   owner/name of the phone app (default below)
# Needs: gh.
#
# Exit 0 when MIN_HUB_CONTRACT <= CONTRACT_REVISION <= MAX_HUB_CONTRACT, 1
# with the reason on stderr otherwise. An unreadable number refuses: guessing
# would be the same silent skew this check exists to stop — fleet-mobile
# 0.4.8 went out at MAX 7 beside a revision-8 hub and refused it as "this app
# is too old".
#
# Two callers, one rule: release-mobile.sh, at the tag and the fleet-mobile
# commit it pairs; and ci.yml, this PR's own tree against fleet-mobile main,
# so a contract bump meets the phone on its own PR, not at release time.
set -euo pipefail

FLEET_REPO="martin-janci/claude-fleet"
MOBILE_REPO="${FLEET_MOBILE_REPO:-martin-janci/fleet-mobile}"
WIRE=crates/fleet-core/src/wire_contract.rs
MOBILE_FILE=shared/src/commonMain/kotlin/dev/claudefleet/mobile/net/HubContract.kt

die() { echo "check-mobile-contract.sh: $*" >&2; exit 1; }

hub_ref="" mobile_ref=main print=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --hub-ref) hub_ref="${2:?--hub-ref needs a ref}"; shift 2 ;;
    --mobile-ref) mobile_ref="${2:?--mobile-ref needs a ref}"; shift 2 ;;
    --print-hub-rev) print=1; shift ;;
    *) die "usage: scripts/check-mobile-contract.sh [--hub-ref <ref>] [--mobile-ref <ref>] [--print-hub-rev]" ;;
  esac
done

file_at() { # file_at <repo> <path> <ref>
  gh api -H "Accept: application/vnd.github.raw" "repos/$1/contents/$2?ref=$3" 2>/dev/null || true
}

if [[ -n "$hub_ref" ]]; then
  hub_src="claude-fleet $hub_ref"
  hub_text="$(file_at "$FLEET_REPO" "$WIRE" "$hub_ref")"
else
  root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
  hub_src="this checkout"
  hub_text="$(cat "$root/$WIRE" 2>/dev/null || true)"
fi
HUB_REV="$(sed -nE 's/^pub const CONTRACT_REVISION: u32 = ([0-9]+);.*/\1/p' <<<"$hub_text")"
[[ "$HUB_REV" =~ ^[0-9]+$ ]] || die "could not read CONTRACT_REVISION from $hub_src ($WIRE)"
if [[ -n "$print" ]]; then echo "$HUB_REV"; exit 0; fi

mobile_label="$mobile_ref"
[[ "$mobile_ref" =~ ^[0-9a-f]{40}$ ]] && mobile_label="${mobile_ref:0:12}"
mobile_text="$(file_at "$MOBILE_REPO" "$MOBILE_FILE" "$mobile_ref")"
MOBILE_MIN="$(sed -nE 's/^const val MIN_HUB_CONTRACT: Int = ([0-9]+).*/\1/p' <<<"$mobile_text")"
MOBILE_MAX="$(sed -nE 's/^const val MAX_HUB_CONTRACT: Int = ([0-9]+).*/\1/p' <<<"$mobile_text")"
[[ "$MOBILE_MIN" =~ ^[0-9]+$ && "$MOBILE_MAX" =~ ^[0-9]+$ ]] \
  || die "could not read MIN/MAX_HUB_CONTRACT from $MOBILE_REPO $mobile_label (shared/…/net/HubContract.kt)"

((MOBILE_MIN <= HUB_REV && HUB_REV <= MOBILE_MAX)) \
  || die "$MOBILE_REPO $mobile_label accepts hub contracts $MOBILE_MIN..$MOBILE_MAX, but $hub_src ships hub contract $HUB_REV — the app would refuse its own hub; bump fleet-mobile's HubContract.kt first"
echo "$MOBILE_REPO $mobile_label: accepts hub contract $HUB_REV ($MOBILE_MIN..$MOBILE_MAX)"
