#!/usr/bin/env bash
# update-channels.sh — write the signed channel documents on the
# `update-channels` branch (update-channel design §5, U10, slice S2).
#
#   update-channels.sh add <tag>
#       list a PUBLISHED release: stable → stable.json and beta.json (the beta
#       track follows every stable release too), -rc.N → beta.json,
#       -dev.N… → dev.json, and nightly.json too when nightly last moved two
#       hours ago or more (NIGHTLY_EVERY_SECS, default 7200). Run by
#       release.yml after `publish`.
#   update-channels.sh edit <track> <op> [version] [component] [reason] [deadline]
#       a publisher's change: withdraw | recommend | rollback | clear-rollback
#       | minimum | clear-minimum | mandatory. Run by update-channels.yml.
#   update-channels.sh amend <tag> <amendment.json>
#       a signed addition to a listed release (design §4, §13.2): the phone's
#       APK from fleet-mobile's release. Written to amendments/<version>/ on
#       this branch, signed, and listed on every track that carries the
#       release. Run by android-amendment.yml.
#   update-channels.sh resign
#       re-sign every track with a fresh sequence and expiry, so a quiet
#       channel never goes stale (14 days). Run weekly by update-channels.yml.
#
#   env: RELEASE_SIGNING_KEY FLEET_RELEASE
#        REPO (owner/repo) GH_TOKEN — `add` downloads the release's manifest
#        BRANCH (default update-channels) REMOTE (default origin)
#        DRY_RUN=1 — commit locally, push nothing
#
# Run from a checkout whose REMOTE can push BRANCH (actions/checkout's own
# credentials). The branch is an orphan holding only the channel documents;
# it is created on the first run. Every document is signed and the signature
# checked against crates/fleet-update/src/keys.rs before anything is pushed.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
: "${FLEET_RELEASE:?FLEET_RELEASE is required (the fleet-release binary)}"
branch="${BRANCH:-update-channels}"
remote="${REMOTE:-origin}"
cmd="${1:?usage: update-channels.sh add <tag> | edit <track> <op> … | resign}"
shift

if [ -z "${RELEASE_SIGNING_KEY:-}" ]; then
  echo "::error title=no release signing key::RELEASE_SIGNING_KEY is not set; run scripts/release-key.sh once (docs/RELEASING.md)."
  exit 1
fi

work="$(mktemp -d)"
tree="$work/tree"
cleanup() {
  git worktree remove --force "$tree" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT

"$here/release-pubkeys.sh" >"$work/pubkeys"
[ -s "$work/pubkeys" ] || { echo "::error::crates/fleet-update/src/keys.rs lists no release key"; exit 1; }
(umask 077 && printf '%s\n' "$RELEASE_SIGNING_KEY" >"$work/key")

# The branch, as a worktree: existing, or a new orphan.
if git fetch -q "$remote" "refs/heads/$branch:refs/remotes/$remote/$branch" 2>/dev/null; then
  git worktree add -q -B "$branch" "$tree" "$remote/$branch"
else
  git worktree add -q --detach "$tree"
  git -C "$tree" checkout -q --orphan "$branch"
  git -C "$tree" rm -rq --cached . >/dev/null 2>&1 || true
  git -C "$tree" clean -fdxq
  cat >"$tree/README.md" <<'EOF'
# update-channels

The signed release channels of claude-fleet (`<track>.json` +
`<track>.json.minisig`), written only by CI: `release.yml` after a release is
published, `update-channels.yml` for edits and the weekly re-sign. Never edit
by hand — a document whose signature does not verify is not a channel.
Design: `docs/superpowers/specs/2026-09-28-update-channel-design.md` §5.
EOF
fi

changed=()
sign() {
  local track="$1" f="$tree/$1.json"
  local seq
  seq="$(sed -n 's/^  "sequence": \([0-9]*\),$/\1/p' "$f" | head -n1)"
  minisign -S -s "$work/key" -m "$f" -x "$f.minisig" -t "claude-fleet $track channel #$seq" >/dev/null
  "$here/release-verify-sig.sh" "$work/pubkeys" "$f" "$f.minisig" channel "$track"
  changed+=("$track#$seq")
}

case "$cmd" in
  add)
    tag="${1:?usage: update-channels.sh add <tag>}"
    version="${tag#v}"
    : "${REPO:?REPO is required for add}"
    gh release download "$tag" --repo "$REPO" -p release-manifest.json -p release-manifest.json.minisig -D "$work" --clobber
    "$here/release-verify-sig.sh" "$work/pubkeys" "$work/release-manifest.json" "$work/release-manifest.json.minisig" manifest
    case "$version" in
      *-dev*)
        # Every green push is a dev release; nightly takes one every two
        # hours at most, measured from this branch's own history.
        tracks="dev"
        last="$(git -C "$tree" log -1 --format=%ct --grep='^channel: list .* nightly' -- nightly.json 2>/dev/null || true)"
        if [ -z "$last" ] || [ $(( $(date +%s) - last )) -ge "${NIGHTLY_EVERY_SECS:-7200}" ]; then
          tracks="dev nightly"
        fi
        ;;
      *-*) tracks="beta" ;;
      *) tracks="stable beta" ;;
    esac
    url="https://github.com/$REPO/releases/download/$tag/release-manifest.json"
    for t in $tracks; do
      args=(channel-add --track "$t" --manifest "$work/release-manifest.json" --manifest-url "$url" --out "$tree/$t.json")
      [ -f "$tree/$t.json" ] && args+=(--channel "$tree/$t.json")
      # nightly.yml keeps only the newest dev releases and those nightly
      # lists; neither track lists more than 15.
      case "$t" in nightly | dev) args+=(--keep 15) ;; esac
      "$FLEET_RELEASE" "${args[@]}"
      sign "$t"
    done
    msg="channel: list $tag on $tracks"
    ;;
  edit)
    track="${1:?usage: update-channels.sh edit <track> <op> [version] [component] [reason] [deadline]}"
    op="${2:?op is required}"
    [ -f "$tree/$track.json" ] || { echo "::error::there is no $track channel yet"; exit 1; }
    args=(channel-edit --track "$track" --channel "$tree/$track.json" --op "$op" --out "$tree/$track.json")
    [ -n "${3:-}" ] && args+=(--version "$3")
    [ -n "${4:-}" ] && args+=(--component "$4")
    [ -n "${5:-}" ] && args+=(--reason "$5")
    [ -n "${6:-}" ] && args+=(--deadline "$6")
    "$FLEET_RELEASE" "${args[@]}"
    sign "$track"
    msg="channel: $op on $track${3:+ ($3)}"
    ;;
  amend)
    tag="${1:?usage: update-channels.sh amend <tag> <amendment.json>}"
    file="${2:?usage: update-channels.sh amend <tag> <amendment.json>}"
    version="${tag#v}"
    : "${REPO:?REPO is required for amend}"
    component="$(python3 -c 'import json,sys; c=json.load(open(sys.argv[1]))["components"]; print(next(iter(c)))' "$file")"
    case "$component" in *[!a-z]* | '') echo "::error::odd component '$component' in $file"; exit 1 ;; esac
    dir="$tree/amendments/$version"
    mkdir -p "$dir"
    cp "$file" "$dir/$component.json"
    minisign -S -s "$work/key" -m "$dir/$component.json" -x "$dir/$component.json.minisig" \
      -t "claude-fleet $tag $component amendment" >/dev/null
    "$here/release-verify-sig.sh" "$work/pubkeys" "$dir/$component.json" "$dir/$component.json.minisig" amendment
    url="https://raw.githubusercontent.com/$REPO/$branch/amendments/$version/$component.json"
    for f in "$tree"/*.json; do
      [ -e "$f" ] || continue
      t="$(basename "$f" .json)"
      # Only the tracks that list the release.
      grep -q "\"version\": \"$version\"" "$f" || continue
      "$FLEET_RELEASE" channel-amend --track "$t" --channel "$f" --amendment "$dir/$component.json" \
        --amendment-url "$url" --out "$f"
      sign "$t"
    done
    [ "${#changed[@]}" -gt 0 ] || { echo "::error::no track lists $version; nothing to amend"; exit 1; }
    msg="channel: $component amendment of $tag"
    ;;
  resign)
    for f in "$tree"/*.json; do
      [ -e "$f" ] || continue
      t="$(basename "$f" .json)"
      "$FLEET_RELEASE" channel-edit --track "$t" --channel "$f" --op resign --out "$f"
      sign "$t"
    done
    msg="channel: re-sign"
    ;;
  *)
    echo "update-channels.sh: unknown command '$cmd'" >&2
    exit 2
    ;;
esac

if [ "${#changed[@]}" -eq 0 ]; then
  echo "update-channels.sh: no channel to write." >&2
  exit 0
fi
git -C "$tree" add -A
git -C "$tree" -c user.name="github-actions[bot]" \
  -c user.email="41898282+github-actions[bot]@users.noreply.github.com" \
  commit -q -m "$msg" -m "${changed[*]}"
if [ "${DRY_RUN:-0}" = 1 ]; then
  echo "update-channels.sh: DRY_RUN — committed ${changed[*]} on $branch locally, pushed nothing." >&2
  exit 0
fi
git -C "$tree" push -q "$remote" "HEAD:refs/heads/$branch"
echo "update-channels.sh: pushed ${changed[*]} to $branch." >&2
