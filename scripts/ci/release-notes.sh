#!/usr/bin/env bash
# Print the GitHub release notes for a tag: a fixed header (downloads, verification,
# prerequisites, upgrade notes) followed by the tag's CHANGELOG.md section.
#
# Usage: scripts/ci/release-notes.sh vX.Y.Z[-rc.N]
# Environment: GITHUB_REPOSITORY (default tinof/hdr-analyze), RUSTC_VERSION (optional,
# the `rustc --version` line the binaries were built with).
set -euo pipefail

tag=${1:?usage: $0 vX.Y.Z[-rc.N]}
cd "$(dirname "$0")/../.."

if [[ ! $tag =~ ^v([0-9]+\.[0-9]+\.[0-9]+)(-rc\.[0-9]+)?$ ]]; then
    echo "error: tag '$tag' is not vX.Y.Z or vX.Y.Z-rc.N" >&2
    exit 1
fi
base=${BASH_REMATCH[1]}
repo=${GITHUB_REPOSITORY:-tinof/hdr-analyze}

# Relative links in CHANGELOG.md (docs/X.md) would resolve against the release page; point them
# at the tagged tree instead.
section=$(awk -v hdr="## [$base]" '
    index($0, hdr) == 1 { on = 1; next }
    on && /^## \[/ { exit }
    on { print }' CHANGELOG.md |
    sed -E "s#\]\((docs/|scripts/|tools/|[A-Za-z_]+\.md)#](https://github.com/$repo/blob/$tag/\1#g")
if [[ -z ${section//[[:space:]]/} ]]; then
    echo "error: CHANGELOG.md has no section '## [$base]'" >&2
    exit 1
fi

if [[ $tag == *-* ]]; then
    cat <<EOF
> **Pre-release.** This is a release candidate for v$base. \`install.sh\` keeps installing the
> latest stable release; download the archive below to try this one.

EOF
fi

cat <<EOF
## HDR-Analyze Suite $tag

| Tool | Description |
|------|-------------|
| \`hdr_analyzer_mvp\` | HDR10/HLG frame analyzer and measurement generator |
| \`mkvdovi\` | HDR10/HDR10+ to Profile 8.1 and HLG to Profile 8.4 converter (CM v4.0 metadata) |
| \`verifier\` | madVR measurement file verification utility |

### Download

| Platform | Archive |
|----------|---------|
| Windows (x64) | \`hdr-analyze-$tag-x86_64-pc-windows-msvc.zip\` |
| macOS (Apple Silicon) | \`hdr-analyze-$tag-aarch64-apple-darwin.tar.gz\` |
| macOS (Intel) | \`hdr-analyze-$tag-x86_64-apple-darwin.tar.gz\` |
| Linux (x64) | \`hdr-analyze-$tag-x86_64-unknown-linux-gnu.tar.gz\` |
| Linux (ARM64) | \`hdr-analyze-$tag-aarch64-unknown-linux-gnu.tar.gz\` |

Linux/macOS one-line install of the latest stable release:

\`\`\`bash
curl -fsSL https://github.com/$repo/releases/latest/download/install.sh | bash
\`\`\`

Windows: extract the zip and keep the FFmpeg DLLs next to the \`.exe\` files. \`install.ps1\` is
attached but not yet tested on Windows.

**Verify a download:** \`sha256sum -c SHA256SUMS --ignore-missing\`, and check that GitHub Actions
built it from this repository: \`gh attestation verify <archive> -R $repo\`.

### Prerequisites

- **FFmpeg** in PATH (the Windows zip bundles the LGPL FFmpeg libraries the analyzer links
  against; \`mkvdovi\` still calls the \`ffmpeg\` command-line tool).
- **mkvdovi** also needs \`mkvmerge\`, \`dovi_tool\` 2.3.2 or newer (2.3.4+ recommended), and
  \`hdr10plus_tool\` for HDR10+ sources.
- Release binaries analyze on the CPU. GPU analysis needs a build with \`--features cuda\`.

### Upgrading

- **Replace all three binaries together.** \`mkvdovi\` checks the analyzer's version and \`--help\`
  text, and the L1 sidecar format it writes.
- **Finish running conversions before you upgrade.** A \`mkvdovi_temp_*\` directory left by another
  version (0.4.0 or later) is not resumed; the conversion starts again. A directory from an older
  version is resumed with a warning: delete it or pass \`--no-resume\`.

## Changes in $base
$section
EOF

if [[ -n ${RUSTC_VERSION:-} ]]; then
    printf "\n---\n\nBuilt with \`%s\`.\n" "$RUSTC_VERSION"
fi
