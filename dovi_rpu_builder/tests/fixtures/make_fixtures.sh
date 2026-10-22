#!/usr/bin/env bash
# Regenerate the dovi_rpu_builder fixtures from dovi_tool 2.3.4: every expected RPU here is
# written by dovi_tool (and, for the bt2100 and canary composers, by fit_hlg_composer), never by
# the library under test.
#
#   dovi_rpu_builder/tests/fixtures/make_fixtures.sh
#
# Writes, next to this script:
#   <case>.extra.json, <case>.RPU.bin        the shot cases of tests/cases/mod.rs
#   profile_{a,b}.payloads, profile_{a,b}.json
#                                           synthetic HDR10+ payloads and their hdr10plus_tool JSON
#   hdr10plus.{v29,v40}.extra.json, profile_{a,b}.<peak>.<cm>.RPU.bin
#                                           dovi_tool generate --hdr10plus-json, per peak source
# Needs: dovi_tool 2.3.4 on PATH, cargo.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)

version=$(dovi_tool --version | awk '{print $2}')
if [[ $version != 2.3.4 ]]; then
    echo "error: dovi_tool $version on PATH; the fixtures pin 2.3.4" >&2
    exit 2
fi

cargo build -q --release --manifest-path "$root/tools/rpu_builder_check/Cargo.toml"
cargo build -q --release --manifest-path "$root/tools/fit_hlg_composer/Cargo.toml"
check="$root/tools/rpu_builder_check/target/release/rpu_builder_check"
fit="$root/tools/fit_hlg_composer/target/release/fit_hlg_composer"

rm -f "$here"/*.extra.json "$here"/*.RPU.bin "$here"/*.payloads "$here"/profile_*.json
"$check" extra-json "$here"
"$check" hdr10plus-fixtures "$here"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

for config in "$here"/*.extra.json; do
    name=$(basename "$config" .extra.json)
    [[ $name == hdr10plus.* ]] && continue
    dovi_tool generate -j "$config" --rpu-out "$work/$name.bin" > /dev/null
    case $name in
        *_bt2100*)
            "$fit" rewrite-rpu bt2100 "$work/$name.bin" > /dev/null
            ;;
        *_canary*)
            # The canary is the bt2100 RPU with the chroma constants shifted (default offset).
            "$fit" rewrite-rpu bt2100 "$work/$name.bin" > /dev/null
            "$fit" canary-rpu "$work/$name.bin" "$work/$name.canary.bin" > /dev/null
            mv "$work/$name.canary.bin" "$work/$name.bin"
            ;;
    esac
    mv "$work/$name.bin" "$here/$name.RPU.bin"
done

for sequence in profile_a profile_b; do
    for cm in v29 v40; do
        for peak in histogram histogram99 max-scl max-scl-luminance; do
            dovi_tool generate -j "$here/hdr10plus.$cm.extra.json" \
                --hdr10plus-json "$here/$sequence.json" --hdr10plus-peak-source "$peak" \
                --rpu-out "$here/$sequence.$peak.$cm.RPU.bin" > /dev/null
        done
    done
done

{
    echo "dovi_tool $version"
    sha256sum "$here"/*.RPU.bin | sed "s#$here/##"
} > "$here/SHA256SUMS"
echo "fixtures written to $here"
