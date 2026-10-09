#!/usr/bin/env bash
# release-update-scripts-test.sh — the update-publishing scripts end to end
# (slice S2), with a throwaway key, a fake `gh` and a local bare remote:
# release-manifest.sh → update-channels.sh add / edit / resign, every
# document checked by the fleet's own verifier (`fleet-release verify`).
#
#   FLEET_RELEASE=target/debug/fleet-release FLEET_HUB=target/debug/fleet-hub \
#     scripts/release-update-scripts-test.sh
#
# Needs minisign, git, tar. Run by ci.yml's hub-headless job.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
FLEET_RELEASE="$(cd "$(dirname "${FLEET_RELEASE:?}")" && pwd)/$(basename "$FLEET_RELEASE")"
FLEET_HUB="$(cd "$(dirname "${FLEET_HUB:?}")" && pwd)/$(basename "$FLEET_HUB")"
export FLEET_RELEASE
command -v minisign >/dev/null || { echo "release-update-scripts-test: minisign not found" >&2; exit 1; }

t="$(mktemp -d)"
trap 'rm -rf "$t"' EXIT
fails=0
check() {
  local what="$1"
  shift
  if "$@"; then echo "  ok    $what"; else echo "  FAIL  $what"; fails=$((fails + 1)); fi
}
json() { # json <file> <python expression over d>
  python3 -c "import json,sys; d=json.load(open(sys.argv[1])); print($2)" "$1"
}

# A throwaway key the scripts trust through a copy of keys.rs.
minisign -G -W -f -p "$t/test.pub" -s "$t/test.key" >/dev/null
pub="$(sed -n 2p "$t/test.pub")"
printf 'pub const RELEASE_KEYS: &[&str] = &[\n    "%s",\n];\n' "$pub" >"$t/keys.rs"
export FLEET_RELEASE_KEYS_RS="$t/keys.rs"
export RELEASE_SIGNING_KEY
RELEASE_SIGNING_KEY="$(cat "$t/test.key")"
printf '%s\n' "$pub" >"$t/pubkeys"

# The release's assets, with the real fleet-hub inside its tarball.
v=0.4.1
assets="$t/assets"
mkdir -p "$assets" "$t/pack"
for n in "claude-fleet_${v}_aarch64.dmg" "claude-fleet_${v}_x64.dmg" "claude-fleet_${v}_amd64.deb" \
  "claude-fleet_${v}_amd64.AppImage" "claude-fleet_${v}_x64-setup.exe" "claude-fleet_${v}_aarch64.app.tar.gz" \
  "fleet-agent-${v}-x86_64-unknown-linux-gnu.tar.gz"; do
  head -c 64 /dev/urandom >"$assets/$n"
done
mkdir -p "$t/pack/fleet-hub-$v"
cp "$FLEET_HUB" "$t/pack/fleet-hub-$v/fleet-hub"
tar -czf "$assets/fleet-hub-${v}-x86_64-unknown-linux-gnu.tar.gz" -C "$t/pack" "fleet-hub-$v"

# A fake gh: the release body carries the hub image block; `release download`
# serves what "the release" holds ($t/published/<tag>).
digest="sha256:$(printf 'hub' | sha256sum | cut -d' ' -f1)"
mkdir -p "$t/bin" "$t/published"
cat >"$t/bin/gh" <<EOF
#!/usr/bin/env bash
case "\$1 \$2" in
  "api repos/"*)
    printf '%s\n' 'notes' '<!-- fleet-hub-image -->' '### Hub image' '' '\`ghcr.io/o/fleet-hub:$v\` — digest \`$digest\`' '<!-- /fleet-hub-image -->' ;;
  "release download")
    tag="\$3"; shift 3; dir=.
    while [ \$# -gt 0 ]; do case "\$1" in -D) dir="\$2"; shift 2;; *) shift;; esac; done
    cp "$t/published/\$tag/"* "\$dir/" ;;
  "secret set") cat >"$t/secret-\$3" ;;
  *) echo "fake gh: \$*" >&2; exit 1 ;;
esac
EOF
chmod +x "$t/bin/gh"
export PATH="$t/bin:$PATH"

echo "release-manifest.sh"
(cd "$t" && TAG="v0.4.0" ASSETS_DIR="$assets" "$here/release-manifest.sh" 2>/dev/null)
check "an older version needs no manifest" test ! -e "$t/release-manifest.json"
check "no key fails the release" bash -c "cd '$t' && ! RELEASE_SIGNING_KEY= TAG=v$v ASSETS_DIR='$assets' '$here/release-manifest.sh' >/dev/null 2>&1"
(cd "$t" && DRY_RUN=1 TAG="v$v" RELEASE_ID=1 REPO=o/r GIT_SHA=abc BUILD_ID="rel-v$v-abc" \
  HUB_IMAGE_WAIT_SECS=0 ASSETS_DIR="$assets" "$here/release-manifest.sh" 2>/dev/null)
m="$t/release-manifest.json"
check "the manifest verifies with the fleet's verifier" \
  "$FLEET_RELEASE" verify --keys-file "$t/pubkeys" --kind manifest --file "$m" --sig "$m.minisig"
contract="$("$FLEET_HUB" compat | python3 -c 'import json,sys; print(json.load(sys.stdin)["contract"]["hub_serves"])')"
check "it states the shipped hub's contract" test "$(json "$m" 'd["compatibility"]["contract"]["hub_serves"]')" = "$contract"
dmin="$(sed -n 's/^pub const MIN_HUB_CONTRACT: u32 = \([0-9]*\);.*/\1/p' "$root/src-tauri/src/backend/contract.rs")"
check "and the desktop's window from contract.rs" test "$(json "$m" 'd["compatibility"]["contract"]["desktop_accepts"][0]')" = "$dmin"
check "it carries the hub image by digest" test "$(json "$m" '[a["digest"] for a in d["components"]["hub"]["artifacts"] if a["kind"]=="oci"][0]')" = "$digest"
check "desktop: two dmgs and a deb to download, three updater bundles" test "$(json "$m" 'len(d["components"]["desktop"]["artifacts"])')" = 6
check "the updater bundles are tauri artifacts" test "$(json "$m" 'sorted(a["name"] for a in d["components"]["desktop"]["artifacts"] if a["kind"]=="tauri")')" = \
  "['claude-fleet_${v}_aarch64.app.tar.gz', 'claude-fleet_${v}_amd64.AppImage', 'claude-fleet_${v}_x64-setup.exe']"
json "$m" '[a["tauri_signature"] for a in d["components"]["desktop"]["artifacts"] if a["name"].endswith(".app.tar.gz")][0]' \
  | base64 -d >"$t/mac.minisig"
check "whose signature is the release key's over the bundle" \
  minisign -V -q -p "$t/test.pub" -m "$assets/claude-fleet_${v}_aarch64.app.tar.gz" -x "$t/mac.minisig"
check "and names the version, as requireSignedVersion wants" grep -q "version:$v" "$t/mac.minisig"

# Publish it, then run update-channels.sh against a local bare remote.
mkdir -p "$t/published/v$v"
cp "$m" "$m.minisig" "$t/published/v$v/"
git init -q --bare "$t/remote.git"
git init -q "$t/repo"
git -C "$t/repo" -c user.name=t -c user.email=t@t commit -q --allow-empty -m init
git -C "$t/repo" remote add origin "$t/remote.git"
chan() { # chan <track> — the pushed document, verified
  git -C "$t/remote.git" show "update-channels:$1.json" >"$t/$1.json"
  git -C "$t/remote.git" show "update-channels:$1.json.minisig" >"$t/$1.json.minisig"
  "$FLEET_RELEASE" verify --keys-file "$t/pubkeys" --kind channel --track "$1" --file "$t/$1.json" --sig "$t/$1.json.minisig" >/dev/null 2>&1
}
uc() { (cd "$t/repo" && REPO=o/r "$here/update-channels.sh" "$@" 2>/dev/null); }

echo "update-channels.sh"
uc add "v$v"
check "a stable release lands on stable" chan stable
check "and on beta" chan beta
check "stable recommends it" test "$(json "$t/stable.json" 'd["recommended"]')" = "$v"

rc="0.4.2-rc.1"
rcassets="$t/assets-rc"
mkdir -p "$rcassets" "$t/published/v$rc" "$t/pack/fleet-hub-$rc"
cp "$FLEET_HUB" "$t/pack/fleet-hub-$rc/fleet-hub"
tar -czf "$rcassets/fleet-hub-${rc}-x86_64-unknown-linux-gnu.tar.gz" -C "$t/pack" "fleet-hub-$rc"
(cd "$t" && DRY_RUN=1 TAG="v$rc" HUB_IMAGE_WAIT_SECS=0 ASSETS_DIR="$rcassets" "$here/release-manifest.sh" 2>/dev/null)
check "an rc's manifest is on the beta track" test "$(json "$t/release-manifest.json" 'd["release"]["track"]')" = beta
cp "$t/release-manifest.json" "$t/release-manifest.json.minisig" "$t/published/v$rc/"
uc add "v$rc"
chan beta
check "an rc lands on beta" test "$(json "$t/beta.json" 'd["current"]')" = "$rc"
chan stable
check "and not on stable" test "$(json "$t/stable.json" 'd["current"]')" = "$v"

# A per-push dev release (nightly.yml): here the hub tarball only. It lands on
# dev, and on nightly when nightly has not moved for two hours.
nv="0.4.2-dev.7.gabc1234"
check "a per-push nightly builds no desktop legs" test "$("$here/release-assets.sh" has-desktop "$nv")" = false
check "a daily nightly does" test "$("$here/release-assets.sh" has-desktop "0.4.2-dev.8.desktop.gabc1234")" = true
nassets="$t/assets-nightly"
mkdir -p "$nassets" "$t/published/v$nv" "$t/pack/fleet-hub-$nv"
cp "$FLEET_HUB" "$t/pack/fleet-hub-$nv/fleet-hub"
tar -czf "$nassets/fleet-hub-${nv}-x86_64-unknown-linux-gnu.tar.gz" -C "$t/pack" "fleet-hub-$nv"
(cd "$t" && DRY_RUN=1 TAG="v$nv" HUB_IMAGE_WAIT_SECS=0 ASSETS_DIR="$nassets" "$here/release-manifest.sh" 2>/dev/null)
check "a nightly's manifest is on the nightly track" test "$(json "$t/release-manifest.json" 'd["release"]["track"]')" = nightly
check "and carries no desktop artifact" test "$(json "$t/release-manifest.json" 'len(d["components"].get("desktop", {}).get("artifacts", []))')" = 0
cp "$t/release-manifest.json" "$t/release-manifest.json.minisig" "$t/published/v$nv/"
before_beta="$(git -C "$t/remote.git" show update-channels:beta.json)"
uc add "v$nv"
check "a dev release lands on dev" chan dev
check "dev lists it as current" test "$(json "$t/dev.json" 'd["current"]')" = "$nv"
check "the first one lands on nightly too" chan nightly
check "nightly lists it as current" test "$(json "$t/nightly.json" 'd["current"]')" = "$nv"
check "and beta did not move" test "$(git -C "$t/remote.git" show update-channels:beta.json)" = "$before_beta"

# The next push, minutes later: dev moves, nightly waits for its two hours.
nv2="0.4.2-dev.9.gabc1235"
nassets2="$t/assets-nightly2"
mkdir -p "$nassets2" "$t/published/v$nv2" "$t/pack/fleet-hub-$nv2"
cp "$FLEET_HUB" "$t/pack/fleet-hub-$nv2/fleet-hub"
tar -czf "$nassets2/fleet-hub-${nv2}-x86_64-unknown-linux-gnu.tar.gz" -C "$t/pack" "fleet-hub-$nv2"
(cd "$t" && DRY_RUN=1 TAG="v$nv2" HUB_IMAGE_WAIT_SECS=0 ASSETS_DIR="$nassets2" "$here/release-manifest.sh" 2>/dev/null)
cp "$t/release-manifest.json" "$t/release-manifest.json.minisig" "$t/published/v$nv2/"
before_nightly2="$(git -C "$t/remote.git" show update-channels:nightly.json)"
uc add "v$nv2"
chan dev
check "the next dev release is dev's current" test "$(json "$t/dev.json" 'd["current"]')" = "$nv2"
check "and nightly did not move within two hours" test "$(git -C "$t/remote.git" show update-channels:nightly.json)" = "$before_nightly2"
(cd "$t/repo" && NIGHTLY_EVERY_SECS=0 REPO=o/r "$here/update-channels.sh" add "v$nv2" 2>/dev/null)
chan nightly
check "once its interval is up, nightly takes the newest" test "$(json "$t/nightly.json" 'd["current"]')" = "$nv2"

# The phone's amendment (android-amendment.yml, design §4 / §13.2): signed,
# kept on the channel branch, listed on every track that carries the release.
"$FLEET_RELEASE" amendment --version "$v" \
  --apk-url "https://github.com/o/fleet-mobile/releases/download/v$v/fleet-mobile-$v.apk" \
  --sha256 "$(printf 'apk' | sha256sum | cut -d' ' -f1)" --size 3 --version-code 41 \
  --signer-sha256 "$(printf 'cert' | sha256sum | cut -d' ' -f1)" --mobile-accepts 0,14 \
  --out "$t/android.json" >/dev/null 2>&1
before_nightly="$(git -C "$t/remote.git" show update-channels:nightly.json)"
uc amend "v$v" "$t/android.json"
chan stable
check "an amendment is listed on stable" test "$(json "$t/stable.json" '[r for r in d["releases"] if r["version"]=="'"$v"'"][0]["amendments"][0]["component"]')" = android
chan beta
check "and on beta, which carries the release too" test "$(json "$t/beta.json" 'len([r for r in d["releases"] if r.get("amendments")])')" = 1
check "but not on nightly, which does not" test "$(git -C "$t/remote.git" show update-channels:nightly.json)" = "$before_nightly"
git -C "$t/remote.git" show "update-channels:amendments/$v/android.json" >"$t/am.json"
git -C "$t/remote.git" show "update-channels:amendments/$v/android.json.minisig" >"$t/am.json.minisig"
check "the amendment is signed by the release key" \
  "$FLEET_RELEASE" verify --keys-file "$t/pubkeys" --kind amendment --file "$t/am.json" --sig "$t/am.json.minisig"
check "and its sha256 is the one the channel lists" test "$(sha256sum "$t/am.json" | cut -d' ' -f1)" = \
  "$(json "$t/stable.json" '[r for r in d["releases"] if r["version"]=="'"$v"'"][0]["amendments"][0]["manifest_sha256"]')"
check "a release no track carries cannot be amended" bash -c "! (cd '$t/repo' && REPO=o/r '$here/update-channels.sh' amend v9.9.9 '$t/android.json' >/dev/null 2>&1)"

seq="$(json "$t/stable.json" 'd["sequence"]')"
uc edit stable minimum "$v" hub
chan stable
check "an edit re-signs with a higher sequence" test "$(json "$t/stable.json" 'd["sequence"]')" -gt "$seq"
check "and sets the signed floor" test "$(json "$t/stable.json" 'd["minimum_supported"]["hub"]')" = "$v"
check "withdrawing the recommended release is refused" bash -c "! (cd '$t/repo' && '$here/update-channels.sh' edit stable withdraw '$v' '' bad >/dev/null 2>&1)"
seq="$(json "$t/stable.json" 'd["sequence"]')"
uc resign
chan stable
check "resign bumps every track" test "$(json "$t/stable.json" 'd["sequence"]')" -gt "$seq"

# A secret that does not match keys.rs pushes nothing.
minisign -G -W -f -p "$t/other.pub" -s "$t/other.key" >/dev/null
before="$(git -C "$t/remote.git" rev-parse update-channels)"
check "a key the fleet does not trust is refused" bash -c "! (cd '$t/repo' && RELEASE_SIGNING_KEY=\"\$(cat '$t/other.key')\" '$here/update-channels.sh' resign >/dev/null 2>&1)"
check "and nothing was pushed" test "$(git -C "$t/remote.git" rev-parse update-channels)" = "$before"

echo "release-key.sh"
# The real keys.rs names the owner's key, so the script runs against an
# empty fixture of keys.rs's shape (the constant as release-key.sh finds it
# before any key, and the fn that reads it).
cat >"$t/keys-empty.rs" <<'RS'
//! The release keys every build trusts (U1).
pub const RELEASE_KEYS: &[&str] = &[];

/// [`RELEASE_KEYS`] as a [`TrustedKeys`](crate::TrustedKeys).
pub fn release_keys() -> crate::TrustedKeys {
    crate::TrustedKeys::from_base64(RELEASE_KEYS.iter().copied())
        .expect("a compiled-in release key must decode")
}
RS
mkdir -p "$t/home"
FLEET_RELEASE_KEYS_RS="$t/keys-empty.rs" HOME="$t/home" USER=t "$here/release-key.sh" --no-keychain --repo o/r >"$t/key.out" 2>&1 || true
newpub="$(FLEET_RELEASE_KEYS_RS="$t/keys-empty.rs" "$here/release-pubkeys.sh")"
check "it writes one public key into keys.rs" test "$(printf '%s\n' "$newpub" | grep -c '^RW')" = 1
check "the secret went to gh on stdin" grep -q 'secret key' "$t/secret-RELEASE_SIGNING_KEY"
check "keys.rs still compiles as the same constant" grep -q '^pub const RELEASE_KEYS: &\[&str\] = &\[$' "$t/keys-empty.rs"
check "and still has the fn that reads it" grep -q '^pub fn release_keys() -> crate::TrustedKeys {$' "$t/keys-empty.rs"
check "a second run is refused (that is a rotation)" bash -c "! FLEET_RELEASE_KEYS_RS='$t/keys-empty.rs' HOME='$t/home' '$here/release-key.sh' --no-keychain >/dev/null 2>&1"

# The build identity (U11, design §8.4 step 3): the tarballs, the hub image
# and the manifest must compile in / sign the SAME build ID, so the three
# workflow lines use one expression, and none names a run or an attempt
# (hub-image.yml is another run; a re-run job is another attempt).
echo "build identity"
wf="$root/.github/workflows"
bid_tar="$(sed -n 's/^ *FLEET_BUILD_ID: //p' "$wf/release.yml")"
bid_man="$(sed -n 's/^ *BUILD_ID: //p' "$wf/release.yml")"
bid_img="$(sed -n 's/^ *FLEET_BUILD_ID=//p' "$wf/hub-image.yml")"
check "release.yml sets FLEET_BUILD_ID once" test "$(printf '%s\n' "$bid_tar" | grep -c .)" = 1
check "release.yml sets the manifest's BUILD_ID once" test "$(printf '%s\n' "$bid_man" | grep -c .)" = 1
check "hub-image.yml sets FLEET_BUILD_ID once" test "$(printf '%s\n' "$bid_img" | grep -c .)" = 1
check "the manifest signs the tarballs' build ID" test "$bid_man" = "$bid_tar"
check "the hub image compiles in the same build ID" test "$bid_img" = "$bid_tar"
# shellcheck disable=SC2016 # the literal workflow expression
check "which names the tag and the commit" test "$bid_tar" = 'rel-${{ github.ref_name }}-${{ github.sha }}'
check "and no run or attempt" bash -c "! grep -nE 'BUILD_ID[:=].*github\.run_(id|attempt)' '$wf/release.yml' '$wf/hub-image.yml'"

if [ "$fails" -ne 0 ]; then
  echo "release-update-scripts-test: $fails check(s) failed" >&2
  exit 1
fi
echo "release-update-scripts-test: all checks passed"
