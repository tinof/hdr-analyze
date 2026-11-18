#!/usr/bin/env bash
# Builds the libplacebo that tools/display_sim links (ROADMAP WS7.1): tag v7.387.0, commit
# 275f7dea27924cdd34ab2e6c20e448b6bc4fab65, as a static library with every GPU and optional
# dependency off. display_sim calls only its tone-mapping curves (src/tone_mapping.c), whose
# numbers the WS7 gate pins; a newer libplacebo can move them, so the version is exact.
#
# Installs into $LIBPLACEBO_PREFIX (default ~/.cache/hdr-analyze/libplacebo-v7.387.0), where
# tools/display_sim/build.rs looks by default. The checkout, the build directory and a private
# Python venv with meson and ninja live beside it in $LIBPLACEBO_WORK (default
# ~/.cache/hdr-analyze/libplacebo-v7.387.0-build), never in the prefix. Needs git, python3 (or
# uv), pkg-config and a C/C++ compiler (gcc or clang); network access on the first run.
#
# Idempotent: when the prefix already holds this version's pkg-config file and static library
# and the stamp names this commit and these options, it only prints the PKG_CONFIG_PATH line.
# The stamp is written last, so an interrupted build is redone. Status goes to stdout, errors and
# the tail of the build log to stderr.
#
# Usage: scripts/build-libplacebo.sh
set -euo pipefail

VERSION=7.387.0
TAG="v$VERSION"
COMMIT=275f7dea27924cdd34ab2e6c20e448b6bc4fab65
REPO="${LIBPLACEBO_REPO:-https://code.videolan.org/videolan/libplacebo.git}"
SUBMODULES=(3rdparty/jinja 3rdparty/markupsafe 3rdparty/fast_float 3rdparty/Vulkan-Headers)
PY_TOOLS=(meson==1.12.1 ninja==1.13.0)
MESON_OPTIONS=(
    --buildtype=release --default-library=static --libdir=lib
    -Dvulkan=disabled -Dopengl=disabled -Dd3d11=disabled -Dglslang=disabled
    -Dshaderc=disabled -Dlcms=disabled -Dlibdovi=disabled -Dunwind=disabled
    -Dxxhash=disabled -Ddemos=false -Dtests=false -Dbench=false -Dfuzz=false
)
CACHE_DIR="$HOME/.cache/hdr-analyze"
PREFIX="${LIBPLACEBO_PREFIX:-$CACHE_DIR/libplacebo-$TAG}"
WORK="${LIBPLACEBO_WORK:-$CACHE_DIR/libplacebo-$TAG-build}"
STAMP="$PREFIX/.hdr-analyze-libplacebo"
STAMP_TEXT="commit=$COMMIT options=${MESON_OPTIONS[*]} tools=${PY_TOOLS[*]}"

die() { echo "build-libplacebo: $*" >&2; exit 1; }
if [ -z "$PREFIX" ] || [ -z "$WORK" ]; then
    die "LIBPLACEBO_PREFIX and LIBPLACEBO_WORK must not be empty"
fi
case "$PREFIX" in /*) ;; *) PREFIX="$PWD/$PREFIX"; STAMP="$PREFIX/.hdr-analyze-libplacebo" ;; esac
case "$WORK" in /*) ;; *) WORK="$PWD/$WORK" ;; esac
for tool in git python3 pkg-config; do
    command -v "$tool" >/dev/null 2>&1 || die "missing tool: $tool"
done

installed() {
    [ -f "$PREFIX/lib/libplacebo.a" ] &&
        [ "$(PKG_CONFIG_LIBDIR="$PREFIX/lib/pkgconfig" pkg-config --modversion libplacebo 2>/dev/null)" = "$VERSION" ] &&
        [ -f "$STAMP" ] && [ "$(command cat "$STAMP")" = "$STAMP_TEXT" ]
}

report() {
    echo "libplacebo $VERSION ($COMMIT) in $PREFIX"
    echo "PKG_CONFIG_PATH=$PREFIX/lib/pkgconfig"
}

if installed; then
    report
    exit 0
fi

mkdir -p "$WORK" "$PREFIX"
LOG="$WORK/build.log"
: > "$LOG"
# Shows the log tail when a step fails.
run() {
    if ! "$@" >> "$LOG" 2>&1; then
        echo "build-libplacebo: failed: $*" >&2
        tail -n 40 "$LOG" >&2
        exit 1
    fi
}

# meson and ninja from a private venv: python3 -m venv with pip, or uv where venv has no pip.
VENV="$WORK/venv"
if ! "$VENV/bin/python" -m mesonbuild.mesonmain --version >/dev/null 2>&1 ||
    ! "$VENV/bin/python" -c 'import ninja' >/dev/null 2>&1; then
    echo "Creating the meson/ninja venv in $VENV"
    rm -rf "$VENV"
    if python3 -m venv "$VENV" >> "$LOG" 2>&1 && [ -x "$VENV/bin/pip" ]; then
        run "$VENV/bin/pip" install --quiet --disable-pip-version-check "${PY_TOOLS[@]}"
    elif command -v uv >/dev/null 2>&1; then
        rm -rf "$VENV"
        run uv venv --quiet "$VENV"
        run uv pip install --quiet --python "$VENV/bin/python" "${PY_TOOLS[@]}"
    else
        die "python3 -m venv gave no pip and uv is not installed; see $LOG"
    fi
fi
export PATH="$VENV/bin:$PATH"

SRC="$WORK/src"
if [ ! -d "$SRC/.git" ]; then
    echo "Cloning libplacebo $TAG"
    rm -rf "$SRC"
    run git clone --quiet --depth 1 --branch "$TAG" "$REPO" "$SRC"
else
    run git -C "$SRC" fetch --quiet --depth 1 origin "refs/tags/$TAG:refs/tags/$TAG"
    run git -C "$SRC" checkout --quiet --detach "$TAG"
fi
head="$(git -C "$SRC" rev-parse 'HEAD^{commit}')"
[ "$head" = "$COMMIT" ] || die "tag $TAG of $REPO is commit $head, not the pinned $COMMIT"
if ! git -C "$SRC" submodule update --init --depth 1 -- "${SUBMODULES[@]}" >> "$LOG" 2>&1; then
    # A server that refuses a shallow fetch of the pinned commit gets the full history.
    run git -C "$SRC" submodule update --init -- "${SUBMODULES[@]}"
fi

echo "Building libplacebo $TAG into $PREFIX"
# An interrupted earlier build must not look complete.
rm -f "$STAMP"
rm -rf "$WORK/build"
run meson setup "$WORK/build" "$SRC" --prefix="$PREFIX" "${MESON_OPTIONS[@]}"
run meson compile -C "$WORK/build"
run meson install -C "$WORK/build"
[ -f "$PREFIX/lib/libplacebo.a" ] || die "no $PREFIX/lib/libplacebo.a after the install; see $LOG"
version="$(PKG_CONFIG_LIBDIR="$PREFIX/lib/pkgconfig" pkg-config --modversion libplacebo || true)"
[ "$version" = "$VERSION" ] || die "the installed libplacebo.pc says '$version', not $VERSION"
printf '%s' "$STAMP_TEXT" > "$STAMP"
report
