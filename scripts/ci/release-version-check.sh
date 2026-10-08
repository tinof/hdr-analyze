#!/usr/bin/env bash
# Check that a release tag matches the shipped crates, Cargo.lock and CHANGELOG.md.
#
# Usage: scripts/ci/release-version-check.sh vX.Y.Z[-rc.N]
#
# The tag without its "v" must equal the version of every shipped crate exactly, so an rc
# carries its suffix in the manifests too: mkvdovi fingerprints resume directories with its
# version, and an rc and the final release must not share one. CHANGELOG.md needs a dated
# `## [X.Y.Z]` section for the base version, with each `###` heading at most once.
set -euo pipefail

tag=${1:?usage: $0 vX.Y.Z[-rc.N]}
cd "$(dirname "$0")/../.."

if [[ ! $tag =~ ^v([0-9]+\.[0-9]+\.[0-9]+)(-rc\.[0-9]+)?$ ]]; then
    echo "error: tag '$tag' is not vX.Y.Z or vX.Y.Z-rc.N" >&2
    exit 1
fi
version=${tag#v}
base=${BASH_REMATCH[1]}
failed=0

# The crates whose binaries or code ship in the release archives.
for crate in hdr_analyzer_mvp mkvdovi verifier dovi84_composer; do
    manifest=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$crate/Cargo.toml" | head -n 1)
    locked=$(awk -v name="name = \"$crate\"" '
        $0 == name { getline; gsub(/^version = "|"$/, ""); print; exit }' Cargo.lock)
    if [[ $manifest != "$version" ]]; then
        echo "error: $crate/Cargo.toml has version $manifest, tag $tag needs $version" >&2
        failed=1
    fi
    if [[ $locked != "$version" ]]; then
        echo "error: Cargo.lock has $crate $locked, tag $tag needs $version (run cargo build)" >&2
        failed=1
    fi
done

if ! grep -Eq "^## \[$base\] - [0-9]{4}-[0-9]{2}-[0-9]{2}$" CHANGELOG.md; then
    echo "error: CHANGELOG.md has no dated section '## [$base] - YYYY-MM-DD'" >&2
    failed=1
else
    duplicates=$(awk -v hdr="## [$base]" '
        index($0, hdr) == 1 { on = 1; next }
        on && /^## \[/ { exit }
        on && /^### / { print }' CHANGELOG.md | sort | uniq -d)
    if [[ -n $duplicates ]]; then
        echo "error: CHANGELOG.md [$base] repeats these headings; merge them:" >&2
        echo "$duplicates" >&2
        failed=1
    fi
fi

if ((failed)); then
    exit 1
fi
echo "release-version-check: $tag matches the crates, Cargo.lock and CHANGELOG.md"
