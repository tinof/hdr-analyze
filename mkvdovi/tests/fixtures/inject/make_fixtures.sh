#!/bin/sh
# Regenerates the fixtures of the in-process RPU injection tests (`mkvdovi/src/inject/tests.rs`,
# `mkvdovi/src/pipeline/tests/injection.rs`). The expected output, BL_RPU.hevc, is written by
# dovi_tool itself, never by mkvdovi. Regenerate only on purpose: x265 and dovi_tool versions
# change the bytes. Last run with FFmpeg n9.0.2 (libx265) and dovi_tool 2.3.4.
#
#   clip.hevc    36 pictures, open GOP (CRA with RASL pictures), B-frames, an AUD per access
#                unit, parameter sets repeated, two slices per picture, a suffix SEI
#                (decoded picture hash) per picture.
#   RPU.bin      36 RPUs from `dovi_tool generate`, each with its own L1, so an RPU written into
#                the wrong picture shows.
#   BL_RPU.hevc  `dovi_tool inject-rpu -i clip.hevc --rpu-in RPU.bin`.
set -eu
cd "$(dirname "$0")"
frames=36
ffmpeg -hide_banner -loglevel error -f lavfi \
    -i "nullsrc=s=256x144:r=24,format=yuv420p10le,geq=lum='200+mod(X*2+N*7\,512)':cb='512+mod(Y+N\,64)':cr='512'" \
    -frames:v "$frames" -c:v libx265 -preset ultrafast \
    -x265-params "log-level=error:keyint=12:min-keyint=12:open-gop=1:bframes=3:b-adapt=0:scenecut=0:aud=1:repeat-headers=1:slices=2:hash=1:colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:range=limited" \
    -pix_fmt yuv420p10le -f hevc -y clip.hevc
shots=
i=0
while [ "$i" -lt "$frames" ]; do
    [ -z "$shots" ] || shots="$shots,"
    shots="$shots{\"start\":$i,\"duration\":1,\"metadata_blocks\":[{\"Level1\":{\"min_pq\":0,\"max_pq\":$((2100 + 25 * i)),\"avg_pq\":$((900 + 10 * i))}}]}"
    i=$((i + 1))
done
printf '{"cm_version":"V40","length":%d,"level6":{"max_display_mastering_luminance":1000,"min_display_mastering_luminance":1,"max_content_light_level":1000,"max_frame_average_light_level":400},"shots":[%s]}\n' \
    "$frames" "$shots" > generate.json
dovi_tool generate -j generate.json -o RPU.bin > /dev/null
dovi_tool inject-rpu -i clip.hevc --rpu-in RPU.bin -o BL_RPU.hevc > /dev/null
rm generate.json
sha256sum clip.hevc RPU.bin BL_RPU.hevc
