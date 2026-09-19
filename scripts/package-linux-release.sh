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
# other's on upload — the published checksums would then cover 2 of the 4
# tarballs, not 4, with nothing to say so. Instead each run writes its own
# `dist/SHA256SUMS.$TARGET`, uploaded as a distinctly-named workflow
# artifact; a separate job (`agent-hub-checksums` in release.yml, see
# scripts/merge-sha256sums.sh) combines both legs' files into the one
# SHA256SUMS the release actually gets.
#
# Run from the repository root, after the cargo build, with:
#   TARGET   the Rust target triple the binaries were built for, e.g.
#            x86_64-unknown-linux-gnu (must match the --target passed to
#            `cargo build`, so `target/$TARGET/release/<bin>` exists)
#   TAG      the release tag, e.g. v0.3.0 (the version in each asset name is
#            this with the leading `v` stripped)
#   REPO     "owner/repo", used only for the README's docs/hub.md link
#
# Every value that could vary per run (tag, target, repo) is read from the
# environment rather than interpolated as workflow YAML text, so the CI job
# passes them through `env:` and never inlines a tag/ref into a shell string.
set -euo pipefail

: "${TARGET:?TARGET is required, e.g. x86_64-unknown-linux-gnu}"
: "${TAG:?TAG is required, e.g. v0.3.0}"
: "${REPO:?REPO is required, e.g. owner/repo}"

version="${TAG#v}"
bin_dir="target/${TARGET}/release"
out="dist"

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
  cat >"$stage/README.txt" <<EOF
$bin $version ($TARGET)

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

ls -la "$out"
