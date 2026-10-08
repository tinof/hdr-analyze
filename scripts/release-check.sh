#!/usr/bin/env bash
# Read-only pre-flight before pushing a release tag. Changes nothing.
#
# Usage: scripts/release-check.sh vX.Y.Z[-rc.N]
#
# Checks: HEAD is origin/main with a clean tree, the tag does not exist yet, versions,
# Cargo.lock and CHANGELOG.md match the tag (scripts/ci/release-version-check.sh), Cargo.lock
# needs no update, and both CI and a Release dry run (`gh workflow run release.yml`) passed on
# HEAD. Lists the local gates CI cannot run.
# The process is in docs/RELEASING.md.
set -uo pipefail

tag=${1:?usage: $0 vX.Y.Z[-rc.N]}
cd "$(dirname "$0")/.." || exit 1
failed=0
ok() { printf '  ok    %s\n' "$1"; }
bad() { printf '  FAIL  %s\n' "$1"; failed=1; }

echo "Release pre-flight for $tag"

git fetch --quiet origin main --tags || bad "git fetch origin failed"
head=$(git rev-parse HEAD)
if [[ $head == "$(git rev-parse origin/main)" ]]; then
    ok "HEAD is origin/main (${head:0:7})"
else
    bad "HEAD ${head:0:7} is not origin/main; tag only what CI tested on main"
fi

if [[ -z $(git status --porcelain --untracked-files=no) ]]; then
    ok "no uncommitted changes to tracked files"
else
    bad "tracked files have uncommitted changes"
fi

if git rev-parse -q --verify "refs/tags/$tag" >/dev/null ||
    [[ -n $(git ls-remote --tags origin "refs/tags/$tag") ]]; then
    bad "tag $tag already exists; never re-tag, release the next version instead"
else
    ok "tag $tag is new"
fi

if version_out=$(scripts/ci/release-version-check.sh "$tag" 2>&1); then
    ok "crate versions, Cargo.lock and CHANGELOG.md match $tag"
else
    bad "version check"
    printf '%s\n' "$version_out" | sed 's/^/        /'
fi

if cargo metadata --locked --format-version 1 >/dev/null 2>&1; then
    ok "Cargo.lock is current"
else
    bad "Cargo.lock needs an update (cargo metadata --locked failed)"
fi

ci=$(gh run list --workflow ci.yml --commit "$head" --json status,conclusion \
    --jq '[.[] | select(.status == "completed")][0].conclusion // "none"' 2>/dev/null || echo "unknown")
if [[ $ci == success ]]; then
    ok "CI passed on ${head:0:7}"
else
    bad "CI on ${head:0:7}: $ci (wait for a green run on main)"
fi

# The release gate (integration tests with their tools, all five builds) runs only in
# release.yml. A dry run on this commit proves it before the tag makes it public.
dry=$(gh run list --workflow release.yml --commit "$head" --json status,conclusion,event \
    --jq '[.[] | select(.status == "completed" and .event != "push")][0].conclusion // "none"' 2>/dev/null || echo "unknown")
if [[ $dry == success ]]; then
    ok "Release dry run passed on ${head:0:7}"
else
    bad "Release dry run on ${head:0:7}: $dry (run: gh workflow run release.yml --ref main, then wait)"
fi

cat <<EOF

Not checked here; run them on the CUDA host when the release changes analysis or mkvdovi:
  scripts/cuda-parity.sh
  scripts/rpu-baseline.sh compare <previous baseline dir>
EOF

if ((failed)); then
    echo
    echo "Pre-flight failed. Fix the FAIL lines before tagging."
    exit 1
fi
cat <<EOF

Pre-flight passed. To release:
  git tag -a $tag -m "$tag" && git push origin $tag
EOF
