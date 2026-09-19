#!/usr/bin/env bash
# Packages the fleet-agent and fleet-hub Linux release binaries that
# `.github/workflows/release.yml`'s `agent-hub-binaries` job just built with
# `cargo build --release --target "$TARGET"`, into the assets that job
# attaches to the draft release: one tarball per binary (binary + LICENSE +
# a one-line README pointing at docs/hub.md) and a SHA256SUMS file covering
# both.
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

# No root LICENSE file exists in this repository yet (every crate's
# Cargo.toml already declares `license = "MIT"`, but nothing carries the
# text) — ship the matching MIT text rather than an archive with no license
# in it at all. If a root LICENSE file is added later, prefer copying that
# one here instead of this embedded copy.
license="$out/LICENSE"
cat >"$license" <<'EOF'
MIT License

Copyright (c) 2026 martin-janci

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
EOF

for bin in fleet-agent fleet-hub; do
  src="$bin_dir/$bin"
  if [ ! -x "$src" ]; then
    echo "package-linux-release.sh: $src not found or not executable" >&2
    exit 1
  fi
  stage="$out/${bin}-${version}-${TARGET}"
  mkdir -p "$stage"
  cp "$src" "$stage/"
  cp "$license" "$stage/LICENSE"
  cat >"$stage/README.txt" <<EOF
$bin $version ($TARGET)

See https://github.com/$REPO/blob/$TAG/docs/hub.md for how to install and
run this binary, including the minimum glibc it needs and how to run it
without systemd.
EOF
  tar -C "$out" -czf "$out/${bin}-${version}-${TARGET}.tar.gz" "$(basename "$stage")"
  rm -rf "$stage"
done

rm -f "$license"
( cd "$out" && sha256sum ./*.tar.gz >SHA256SUMS )

ls -la "$out"
