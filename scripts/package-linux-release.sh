#!/usr/bin/env bash
# Packages the fleet-agent and fleet-hub Linux release binaries that
# `.github/workflows/release.yml`'s `agent-hub-binaries` job just built with
# `cargo build --release --target "$TARGET"`, into the assets that job
# attaches to the draft release: one tarball per binary (binary + a root
# LICENSE, verbatim, if the checkout has one + a one-line README pointing
# at docs/hub.md), and this leg's own per-target checksums file.
#
# This does NOT write a combined SHA256SUMS: two matrix legs (one per
# target) each run this script independently, and a file named the same by
# both would just have whichever leg finishes last silently overwrite the
# other's on upload. Instead each run writes its own
# `dist/SHA256SUMS.$TARGET`, uploaded as a distinctly-named workflow
# artifact. The release's single `SHA256SUMS` is produced later by the
# `checksums` job, from the assets actually attached to the release (so it
# covers the desktop bundles too, not just these tarballs); these per-target
# files become the cross-check that the bytes GitHub serves are the bytes
# this runner built. See scripts/merge-sha256sums.sh.
#
# Run from the repository root, after the cargo build, with:
#   TARGET   the Rust target triple the binaries were built for, e.g.
#            x86_64-unknown-linux-gnu (must match the --target passed to
#            `cargo build`, so `target/$TARGET/release/<bin>` exists)
#   TAG      the release tag, e.g. v0.3.0 (the version in each asset name is
#            this with the leading `v` stripped)
#   REPO     "owner/repo", used only for the README's docs/hub.md link
#   GIT_SHA  the full commit sha this was built from (`github.sha`), recorded
#            verbatim in each tarball's README.txt. Required, not optional: a
#            tag can be moved or re-cut, so the tag alone does not identify
#            the source of a binary somebody downloaded — the sha does, and a
#            tarball that silently omitted it would be indistinguishable from
#            one built from a different commit under the same name.
#
# Every value that could vary per run (tag, target, repo, sha) is read from
# the environment rather than interpolated as workflow YAML text, so the CI
# job passes them through `env:` and never inlines a tag/ref into a shell
# string.
#
# The asset names this writes are not invented here: they are asserted
# against `scripts/release-assets.sh assets <version> bins-<target>` — the
# one manifest that also drives release.yml's matrices and
# scripts/verify-release.sh — so a rename cannot land in one of those places
# and not the others.
set -euo pipefail

: "${TARGET:?TARGET is required, e.g. x86_64-unknown-linux-gnu}"
: "${TAG:?TAG is required, e.g. v0.3.0}"
: "${REPO:?REPO is required, e.g. owner/repo}"
: "${GIT_SHA:?GIT_SHA is required — the commit this was built from}"

here="$(cd "$(dirname "$0")" && pwd)"
version="${TAG#v}"
bin_dir="target/${TARGET}/release"
out="dist"

# crate_version CARGO_TOML -> the `version = "..."` from its [package]
# section, and only that section: a `version` under [dependencies] or any
# other table must not be picked up. POSIX awk, no GNU-only constructs —
# this runs on a Linux runner today, but nothing about the script needs it
# to.
crate_version() {
  awk '
    /^[[:space:]]*\[/ { in_pkg = ($0 ~ /^[[:space:]]*\[package\][[:space:]]*$/); next }
    in_pkg && /^[[:space:]]*version[[:space:]]*=/ {
      # version = "0.2.22"  ->  0.2.22
      line = $0
      sub(/^[^=]*=[[:space:]]*"/, "", line)
      sub(/".*$/, "", line)
      print line
      exit
    }
  ' "$1"
}

# The tag names the asset; CARGO_PKG_VERSION is what the binary inside it
# reports. Nothing made those agree — a re-tag, a hand-made tag, or a run
# against an older tag would produce a mislabelled tarball whose checksums
# all verify. Checked once, before anything is packaged.
for bin in fleet-agent fleet-hub; do
  manifest="crates/$bin/Cargo.toml"
  if [ ! -f "$manifest" ]; then
    echo "package-linux-release.sh: $manifest not found (run from the repository root)" >&2
    exit 1
  fi
  crate_v="$(crate_version "$manifest")"
  if [ "$crate_v" != "$version" ]; then
    echo "package-linux-release.sh: $bin is version '$crate_v' in $manifest," \
         "but TAG=$TAG names version '$version'." >&2
    echo "package-linux-release.sh: the tag and the built binaries disagree;" \
         "re-tag, or build from the commit the tag points at." >&2
    exit 1
  fi
done

rm -rf "$out"
mkdir -p "$out"

# A root LICENSE file, copied verbatim, if and only if this checkout
# actually has one — this script must never fabricate license text (there
# is no root LICENSE in this repository as of this writing; only each
# crate's Cargo.toml declares `license = "MIT"`). Accepts the common
# spellings; whichever exists first, unmodified.
license=""
for candidate in LICENSE LICENSE.md LICENSE.txt; do
  if [ -f "$candidate" ]; then
    license="$candidate"
    break
  fi
done

for bin in fleet-agent fleet-hub; do
  src="$bin_dir/$bin"
  if [ ! -x "$src" ]; then
    echo "package-linux-release.sh: $src not found or not executable" >&2
    exit 1
  fi
  stage="$out/${bin}-${version}-${TARGET}"
  mkdir -p "$stage"
  cp "$src" "$stage/"
  license_line="License: MIT (see the repository)"
  if [ -n "$license" ]; then
    cp "$license" "$stage/LICENSE"
    license_line="License: see LICENSE"
  fi
  # The commit is recorded next to the version because the version alone
  # cannot answer "what is in this tarball": a tag can be re-cut and a
  # version can be bumped by a hand commit, and the binary reports only
  # CARGO_PKG_VERSION. With the sha, anyone holding this tarball can check
  # out exactly what produced it.
  cat >"$stage/README.txt" <<EOF
$bin $version ($TARGET)

Built from https://github.com/$REPO/commit/$GIT_SHA
Tag: $TAG

See https://github.com/$REPO/blob/$TAG/docs/hub.md for how to install and
run this binary, including the minimum glibc it needs and how to run it
without systemd.

$license_line
EOF
  tar -C "$out" -czf "$out/${bin}-${version}-${TARGET}.tar.gz" "$(basename "$stage")"
  rm -rf "$stage"
done

# `--` before the glob, and bare filenames (not `./fleet-...`): asset names
# are validated to start with a letter (fleet-agent/fleet-hub), so a
# leading `-` can't happen, but `--` costs nothing and rules it out for
# good. sha256sum -c expects a bare filename to match a downloaded file in
# the same directory; a `./` prefix works with --ignore-missing too, but
# the requirement is bare names, so this is `*.tar.gz`, not `./*.tar.gz`.
( cd "$out" && sha256sum -- *.tar.gz >"SHA256SUMS.${TARGET}" )

# The tarballs just written must be exactly the ones the release manifest
# says this leg produces — no more, no fewer, no differently spelled. Without
# this, a rename here would silently drop an asset from the release and the
# only symptom would be verify-release.sh failing at the very end of a full
# build; with it, the leg that caused the drift is the leg that fails.
produced="$(cd "$out" && ls -1 ./*.tar.gz | sed 's#^\./##' | LC_ALL=C sort)"
declared="$("$here/release-assets.sh" assets "$version" "bins-${TARGET}" | LC_ALL=C sort)"
if [ "$produced" != "$declared" ]; then
  echo "package-linux-release.sh: the tarballs written disagree with scripts/release-assets.sh" >&2
  echo "package-linux-release.sh: declared for leg bins-${TARGET}:" >&2
  printf '  %s\n' $declared >&2
  echo "package-linux-release.sh: actually produced:" >&2
  printf '  %s\n' $produced >&2
  exit 1
fi

ls -la "$out"
