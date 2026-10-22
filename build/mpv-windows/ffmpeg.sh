#!/bin/bash
# Configures, builds and installs FFmpeg with clang-cl, run by build.py from the build directory under MSYS2's bash.
# MSYS2 provides only the shell and make that FFmpeg's configure and Makefile need; the compiler, linker and libraries are MSVC's
# and clang-cl, so nothing from MSYS2 ends up in the DLLs (check.py verifies the imports).
#
# Licence: no --enable-gpl, --enable-version3 or --enable-nonfree, so FFmpeg reports "LGPL version 2.1 or later" (check.py fails
# otherwise). --disable-autodetect keeps external libraries to the ones named here.
#
# -MD: the shared Universal CRT like every other DLL in the set (clang-cl, like cl, defaults to the static one).
#
# Usage: ffmpeg.sh <ffmpeg source dir> <install prefix> <pkgconf.exe>   (Windows paths with forward slashes)
set -euo pipefail

src=$1
prefix=$2
pkgconf=$3

# MSYS2's tools first: Windows' own find.exe and sort.exe must not shadow them. FFmpeg's mslink wrapper finds MSVC's link.exe
# past coreutils' link.
export PATH="/usr/bin:$PATH"

for tool in make cmp; do
    command -v "$tool" >/dev/null || { echo "ffmpeg.sh: $tool not found in MSYS2 (pacman -S make diffutils)" >&2; exit 1; }
done

"$src/configure" \
    --prefix="$prefix" \
    --toolchain=msvc \
    --cc=clang-cl \
    --cxx=clang-cl \
    --arch=x86_64 \
    --target-os=win64 \
    --pkg-config="$pkgconf" \
    --extra-cflags="-MD -I$prefix/include" \
    --extra-ldflags="-LIBPATH:$prefix/lib" \
    --enable-shared \
    --disable-static \
    --disable-programs \
    --disable-doc \
    --disable-debug \
    --disable-autodetect \
    --enable-w32threads \
    --enable-schannel \
    --enable-zlib \
    --enable-d3d11va \
    --enable-d3d12va \
    --enable-dxva2 \
    --enable-libdav1d \
    --disable-avdevice \
    --disable-devices \
    --disable-encoders \
    --enable-encoder=png,mjpeg \
    --disable-muxers \
    --enable-muxer=spdif

make -j"$(nproc)"
make install
