# Windows libmpv build (D6)

Our own LGPL-2.1 build of libmpv and FFmpeg for Windows x64, made in GitHub Actions (`.github/workflows/mpv-windows.yml`) from the
scripts in `build/mpv-windows/`. It replaces the prebuilt libmpv the D2 spike used (ADR 0007), which included uchardet under MPL-1.1
and LAME. Licence rules: ADR 0003.

## What is built
Every source is pinned in `build/mpv-windows/sources.toml`: release tarballs by SHA-256, and sources that publish no release tarball
(mpv, libplacebo, shaderc and their submodules, SPIRV-Cross, win-iconv) by git commit, fetched at depth 1 and checked against the pinned
commit and tag. Versions were the newest releases on 2026-10-22.

| Component | Version | Licence used | Linked | Why |
|---|---|---|---|---|
| mpv | 0.41.0 | LGPL-2.1-or-later (`-Dgpl=false`) | DLL | The engine. 0.41.0 has `--d3d11-output-mode=composition`, `--d3d11-composition-size` and `--display-fps-override` (its `DOCS/man/options.rst`) |
| FFmpeg | 9.0.2 | LGPL-2.1-or-later | DLLs (avcodec, avformat, avfilter, avutil, swresample, swscale) | Demuxing, decoding, hwdec d3d11va |
| libplacebo | 7.360.1 | LGPL-2.1-or-later | DLL | `vo=gpu-next` with its D3D11 backend |
| shaderc (+ glslang, SPIRV-Tools, SPIRV-Headers at the commits its DEPS file pins) | 2026.4 | Apache-2.0 (glslang: BSD/MIT/Apache-2.0, one Bison-exception file, ADR 0003) | DLLs (`shaderc_shared`, `SPIRV-Tools-shared`; glslang static in shaderc) | GLSL to SPIR-V. mpv's `d3d11` feature requires shaderc and SPIRV-Cross (its `meson.build`) |
| SPIRV-Cross | vulkan-sdk-1.4.363.0 | Apache-2.0 | DLL (`spirv-cross-c-shared`, the name mpv and libplacebo require) | SPIR-V to HLSL for D3D11 |
| libass | 0.17.5 | ISC | static in mpv | ASS/SSA rendering, DirectWrite font provider |
| FreeType | 2.15.0 | FTL (of FTL or GPL-2.0) | static in mpv | Fonts for libass |
| HarfBuzz | 14.6.0 | MIT | static in mpv | Text shaping for libass |
| FriBidi | 1.0.17 | LGPL-2.1-or-later | DLL | Bidirectional text for libass |
| uchardet | 0.0.8 | LGPL-2.1-or-later (of MPL-1.1/GPL-2.0/LGPL-2.1, ADR 0003) | DLL | Subtitle charset detection |
| win-iconv | 0.0.10 | public domain ("win_iconv is placed in the public domain.", `readme.txt`) | static in mpv | iconv over the Win32 API; mpv enables uchardet and `--sub-codepage` conversion only with iconv |
| zlib | 1.3.2 | Zlib | static in FFmpeg and mpv | PNG, compressed Matroska tracks |
| Little CMS | 2.19.1 | MIT | static in libplacebo and mpv | ICC profiles |
| dav1d | 1.5.4 | BSD-2-Clause | static in avcodec | Software AV1 |
| fast_float, Vulkan-Headers | libplacebo's submodule commits | MIT; Apache-2.0 or MIT | headers compiled into libplacebo | libplacebo needs them even with Vulkan disabled |

Build tools, not shipped: LLVM clang and clang-cl with lld-link (on the runner image), MSVC headers and libraries and the Windows SDK
(Visual Studio on the image, through `vcvars64.bat`), CMake and Python (image), meson and ninja (pip, versions pinned in the workflow),
nasm 3.02 (official win64 zip, SHA-256 pinned), pkgconf 3.0.10 (built from source first), jinja and markupsafe (libplacebo's shader
generator), and MSYS2's bash and make (on the image; FFmpeg's configure needs a POSIX shell). The licence check fails if any DLL
imports an MSYS or MinGW runtime.

Linking: every LGPL component is its own DLL, so a user can replace it (ADR 0003); permissive libraries are linked statically into the
DLL that uses them. Everything uses the shared Universal CRT (`/MD`), checked from the imports, so the app must ship or require the
Visual C++ runtime (`vcruntime140.dll`, `msvcp140.dll` for the C++ parts).

## Configuration
- **Compilers:** clang (GNU-style driver, MSVC ABI) with lld-link for the meson and CMake projects, as mpv's own Windows CI builds
  mpv and its dependencies (mpv v0.41.0, `.github/workflows/build.yml`); clang-cl for FFmpeg, whose configure supports the MSVC
  toolchain only through the cl-style driver.
- **mpv:** `--auto-features=disabled`, then `-Dgpl=false -Dlibmpv=true -Dcplayer=false` and only `d3d11, shaderc, spirv-cross,
  d3d-hwaccel, wasapi, win32-threads, iconv, uchardet, lcms2, zlib` enabled; `-Dbuild-date=false`. Off: Lua and JavaScript scripting,
  OpenGL, Vulkan, libarchive, Blu-ray, DVD, CD, JPEG through libjpeg (screenshots use FFmpeg's encoders), and the media-session
  integration (`win32-smtc`; the app does this itself, ADR 0007).
- **FFmpeg:** `--toolchain=msvc --cc=clang-cl --enable-shared --disable-static --disable-programs --disable-doc --disable-autodetect`,
  then explicitly `--enable-w32threads --enable-schannel --enable-zlib --enable-d3d11va --enable-d3d12va --enable-dxva2
  --enable-libdav1d`; `--disable-avdevice --disable-devices --disable-encoders --enable-encoder=png,mjpeg --disable-muxers
  --enable-muxer=spdif`. No `--enable-gpl`, `--enable-version3` or `--enable-nonfree`. All decoders, parsers, demuxers, protocols,
  bitstream filters, filters and hwaccels that need no external library stay on.
- **libplacebo:** D3D11, shaderc, lcms and the built-in Dolby Vision code on; Vulkan, OpenGL, glslang, libdovi, xxhash off.
- **libass:** DirectWrite font provider and nasm assembly on; fontconfig and libunibreak off.

## What is left out, and why
- **Encoders** except PNG and MJPEG: a player encodes only screenshots and thumbnails (mpv's `video/image_writer.c` uses
  `avcodec_find_encoder` for png and jpg). WebP, JPEG XL and AVIF screenshots need external encoders and are unavailable.
- **Muxers** except spdif: mpv's audio passthrough (`audio/decode/ad_spdif.c`) needs it. Without other muxers `--stream-record` and
  AVIF screenshots do not work; mpv's AC-3 encoding filter for S/PDIF (`af_lavcac3enc`) lacks its encoder.
- **External codec libraries:** libx264, libx265, xvid (GPL, and encoders); fdk-aac and other nonfree code; libaom and libvpx (dav1d
  decodes AV1, FFmpeg's own decoders handle VP8/VP9); opencore-amr and other `version3` (LGPL-3.0/Apache-2.0-only) libraries.
- **LAME:** owner's decision, ADR 0003.
- **Network libraries:** TLS is Windows' SChannel only (no OpenSSL, GnuTLS, mbedTLS); no librtmp, libsmbclient (GPL-3.0), libssh,
  SRT, RIST or ZeroMQ. FFmpeg's built-in protocols (file, http, https, tcp, udp, hls via the demuxer, rtmp, rtp and the like) are all
  LGPL; the check lists them and fails on any protocol from an external library.
- **bzip2 and lzma** (rare Matroska compression), **iconv in FFmpeg** (mpv converts subtitles itself), **MediaFoundation**,
  **AMF**, **NVENC/NVDEC/CUDA** and **Vulkan** in FFmpeg: not needed; mpv decodes through d3d11va.
- **libmpv's own Vulkan and OpenGL outputs:** the Windows player uses `vo=gpu-next` with D3D11 only (ADR 0007).

## Licence check
`build/mpv-windows/check.py` runs after the build and fails the workflow unless:
- FFmpeg's configure recorded "LGPL version 2.1 or later", `CONFIG_GPL`, `CONFIG_VERSION3` and `CONFIG_NONFREE` are off, the only
  external library is dav1d, TLS goes through SChannel, and only the encoders and muxers above are enabled;
- `probe.c`, compiled against the output and run, reads "LGPL version 2.1 or later" from all six FFmpeg libraries at run time;
- mpv's `config.h` has `HAVE_GPL 0` and its configuration `-Dgpl=false`, and the features above are on;
- libmpv accepts `vo=gpu-next`, `gpu-api=d3d11`, `gpu-context=d3d11`, `d3d11-output-mode=composition`, `d3d11-composition-size`,
  `display-fps-override` and the colour options of ADR 0007 (mpv validates option values when they are set), initialises headless, and
  lists the decoders and demuxers the player needs (H.264, HEVC, AV1, VP9 and the common audio codecs in mpv's `decoder-list`;
  Matroska, MP4, MPEG-TS, HLS and others in `demuxer-lavf-list`); the subtitle decoders (ASS, SRT, WebVTT, PGS, DVD, DVB, mov_text)
  are looked up in libavcodec by codec, because mpv's `decoder-list` holds audio and video decoders only (it is filled from
  `vd_lavc.c` and `ad_lavc.c`);
- no DLL imports an MSYS or MinGW runtime, and every DLL uses the shared C runtime.

It writes `components.json` and `components.md` (name, version, licence, source and checksum for every component and tool),
`licences/` (each component's licence files, plus the LGPL-2.1 text), `imports.txt`, `probe.txt` and `build-info.json` (configure lines,
tool versions, stage times).

## Output
The workflow artifact `libmpv-windows-x64`: `bin/` with the DLL set (13 DLLs: `mpv-2.dll`; `avcodec-63.dll`, `avfilter-12.dll`,
`avformat-63.dll`, `avutil-61.dll`, `swresample-7.dll`, `swscale-10.dll`; `libplacebo-360.dll`, `fribidi-0.dll`, `uchardet.dll`;
`shaderc_shared.dll` with `SPIRV-Tools-shared.dll` (shaderc's shared build links SPIRV-Tools as its own DLL), and
`spirv-cross-c-shared.dll`), `include/mpv/` and `lib/mpv.lib` for linking, and the files above.

## Build time
BUILD_TIME_PLACEHOLDER

## Rebuilding locally
On Windows 11 x64 with Visual Studio 2022 or later (C++ desktop workload), LLVM (clang, clang-cl, lld-link, llvm-readobj on `PATH`
or in `C:\Program Files\LLVM`), CMake, Git, Python 3.11 or later with `pip install meson==1.12.1 ninja==1.13.2`, and MSYS2 at
`C:\msys64` with `pacman -S make diffutils`:
```
python build\mpv-windows\build.py --work C:\mpvb
python build\mpv-windows\check.py --work C:\mpvb --out C:\mpvb\out
```
`build.py --fetch-only` downloads and verifies the sources without building. A short work path matters: deep build trees exceed
Windows' path length limit. Finished stages leave a stamp in `<work>\prefix\share\current-build\stamps`; delete the prefix to rebuild
from scratch.

## Publishing the corresponding source (ADR 0003)
For each release that ships these DLLs, the release attaches: the unmodified tarballs listed in `sources.toml` (which the build
verified by SHA-256), a `git archive` of each pinned commit for the git sources, our build scripts (`build/mpv-windows/` at the release
commit, which is the complete build configuration; no source is patched), `components.md` and the licence texts. Users can rebuild and
replace any LGPL DLL with the steps above. Making that bundle is a release-workflow task, not part of this one.

## ARM64 (not built)
What it would take: a `windows-11-arm` runner (or cross-compiling from x64 with `--target=aarch64-pc-windows-msvc`), the
`vcvarsarm64` environment, FFmpeg configured with `--arch=aarch64` (its ARM assembly goes through `armasm64.exe` or clang's integrated
assembler via gas-preprocessor), dav1d's ARM assembly (meson cross file), no nasm, and the same licence check run on an ARM64 machine.
Not tried.

## Not verified
- The DLL set is checked headless in CI only; playing video in the app with it is the later `current-mpv` task.
- mpv 0.41.0 builds against FFmpeg 9.0.2 (also checked on Linux in the cloud container, 2026-10-22, with no warnings), but runtime
  playback with this exact build is not tested.
- GitHub source archives could not be downloaded from the cloud container, so git sources are pinned by commit rather than by tarball
  checksum.

## Sources (read 2026-10-22)
- mpv v0.41.0: `DOCS/man/options.rst`, `meson.build`, `meson.options`, `Copyright`, `video/image_writer.c`,
  `audio/decode/ad_spdif.c`, `.github/workflows/build.yml`: https://github.com/mpv-player/mpv/tree/v0.41.0
- FFmpeg 9.0.2 `configure` (licence strings, autodetected libraries, MSVC toolchain): https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz
- libplacebo v7.360.1 `meson_options.txt`, `src/d3d11/meson.build`: https://github.com/haasn/libplacebo/tree/v7.360.1
- shaderc v2026.4 `DEPS`, `CMakeLists.txt`: https://github.com/google/shaderc/tree/v2026.4
- glslang `LICENSE.txt` at e1b562a8: https://github.com/KhronosGroup/glslang
- uchardet 0.0.8 source headers: https://www.freedesktop.org/software/uchardet/releases/uchardet-0.0.8.tar.xz
- win-iconv `readme.txt`, `win_iconv.c`: https://github.com/win-iconv/win-iconv
- GitHub runner image `windows-latest` = Windows Server 2025 with Visual Studio 2026, image 20261007.264.1 (LLVM 20.1.8, CMake 4.4.4,
  Python 3.12, MSYS2 at `C:\msys64` not on `PATH`, no nasm or pkg-config):
  https://github.com/actions/runner-images/blob/main/images/windows/Windows2025-VS2026-Readme.md
