#!/usr/bin/env bash
# Copy the LGPL FFmpeg runtime DLLs that hdr_analyzer_mvp.exe loads, plus the FFmpeg licence,
# into a directory holding the built .exe files, then prove the bundle is self-sufficient by
# running the analyzer with FFmpeg removed from PATH.
#
# Usage: bundle-windows-ffmpeg.sh <ffmpeg-bin-dir> <dest-dir>
# <ffmpeg-bin-dir> is the `bin-dir` output of .github/actions/setup-ffmpeg.
set -euo pipefail

src=$(cygpath -u "$1")
dest=$2

# avcodec/avformat/avutil/swscale are linked directly; swresample is a load-time
# dependency of avcodec.
for lib in avcodec avformat avutil swscale swresample; do
  cp "$src"/"$lib"-*.dll "$dest/"
done
cp "$src/FFMPEG-LICENSE.txt" "$dest/"

# Windows resolves DLLs from the executable's own directory first; with FFmpeg off PATH,
# this fails if any required DLL is missing from the bundle.
(cd "$dest" && PATH=/usr/bin ./hdr_analyzer_mvp.exe --version)
