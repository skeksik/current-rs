#!/usr/bin/env python3
"""Checks the licence configuration of the Windows libmpv build and assembles the output directory.

Fails unless: FFmpeg reports "LGPL version 2.1 or later" (configure and every library at run time) with only the external libraries,
encoders and muxers listed below; mpv was built with -Dgpl=false; mpv accepts the D3D11 composition options of ADR 0007; and no DLL
imports a MinGW or MSYS runtime. Writes the DLL set, the mpv headers and import library, the licence texts, components.json and
components.md to --out. Python standard library only; run after build.py, in the same kind of shell.
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tomllib
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.dont_write_bytecode = True
sys.path.insert(0, str(HERE))
from build import MSYS2_ROOT, msvc_env  # noqa: E402

LGPL = "LGPL version 2.1 or later"
ALLOWED_EXTERNAL_LIBS = {"LIBDAV1D"}
ALLOWED_ENCODERS = {"PNG", "MJPEG"}
ALLOWED_MUXERS = {"SPDIF"}
# Imports that mean a MinGW/MSYS runtime crept in.
FORBIDDEN_IMPORT = re.compile(r"^(msys-.*|cyg.*|libgcc.*|libwinpthread.*|libstdc\+\+.*|libc\+\+.*|libssp.*)\.dll$", re.I)
SYSTEM_IMPORT = re.compile(r"^(api-ms-win-.*|ext-ms-.*|vcruntime140.*|msvcp140.*|kernel32|user32|gdi32|ole32|oleaut32|shell32|advapi32|"
                           r"bcrypt|secur32|crypt32|ncrypt|ws2_32|winmm|avrt|d3d11|d3d12|dxgi|d3dcompiler_47|dwrite|version|ntdll|"
                           r"shlwapi|psapi|dwmapi|imm32|uxtheme|comctl32|comdlg32|setupapi|propsys|mfplat|mf|mfreadwrite|"
                           r"powrprof|userenv|iphlpapi|dbghelp|cfgmgr32|winhttp|wininet)(\.dll)?$", re.I)
# Decoders and demuxers the player needs (ADR 0007, docs/architecture.md). mpv's decoder-list covers audio and video only
# (mp_add_lavc_decoders is called from vd_lavc.c and ad_lavc.c), so probe.c looks subtitle decoders up in libavcodec by name.
REQUIRED_DECODERS = {"h264", "hevc", "av1", "vp9", "vp8", "mpeg2video", "vc1", "aac", "ac3", "eac3", "truehd", "dts", "flac", "opus",
                     "vorbis", "mp3", "pcm_s16le", "mjpeg", "png"}
REQUIRED_SUBTITLE_DECODERS = {"ass", "subrip", "webvtt", "hdmv_pgs_subtitle", "dvd_subtitle", "dvb_subtitle", "mov_text"}
REQUIRED_DEMUXERS = {"matroska,webm", "mov,mp4,m4a,3gp,3g2,mj2", "mpegts", "hls", "flac", "ogg", "avi", "srt", "ass", "webvtt"}

failures = []


def fail(msg):
    print(f"[check] FAIL: {msg}", flush=True)
    failures.append(msg)


def ok(msg):
    print(f"[check] ok: {msg}", flush=True)


def version_of(cmd, env):
    try:
        out = subprocess.run(cmd, capture_output=True, text=True, env=env, timeout=60)
        return (out.stdout or out.stderr).strip().splitlines()[0]
    except (OSError, IndexError, subprocess.SubprocessError):
        return "unknown"


def check_ffmpeg_config(info):
    config_h = (info / "ffmpeg" / "config.h").read_text()
    config_mak = (info / "ffmpeg" / "config.mak").read_text()
    # Since FFmpeg 6.1 the per-component switches (encoders, muxers, protocols) live in config_components.h.
    components_h = (info / "ffmpeg" / "config_components.h").read_text()
    licence = re.search(r'#define FFMPEG_LICENSE "([^"]*)"', config_h).group(1)
    (ok if licence == LGPL else fail)(f"FFmpeg configure licence: {licence}")
    for flag in ("GPL", "VERSION3", "NONFREE"):
        if re.search(rf"^CONFIG_{flag}=yes$", config_mak, re.M):
            fail(f"FFmpeg CONFIG_{flag} is enabled")
    configuration = re.search(r'#define FFMPEG_CONFIGURATION "([^"]*)"', config_h).group(1)
    for bad in ("--enable-gpl", "--enable-version3", "--enable-nonfree", "libmp3lame"):
        if bad in configuration:
            fail(f"FFmpeg configured with {bad}")
    enabled = set(re.findall(r"^#define CONFIG_(\w+) 1$", config_h, re.M))
    components = set(re.findall(r"^#define CONFIG_(\w+) 1$", components_h, re.M))
    libs = {c for c in enabled if c.startswith("LIB")}
    (ok if libs <= ALLOWED_EXTERNAL_LIBS else fail)(f"FFmpeg external libraries: {sorted(libs)}")
    for tls in ("OPENSSL", "GNUTLS", "LIBTLS", "MBEDTLS"):
        if tls in enabled:
            fail(f"FFmpeg uses {tls}; only SChannel is allowed")
    (ok if "SCHANNEL" in enabled else fail)("FFmpeg TLS through SChannel")
    encoders = {c[: -len("_ENCODER")] for c in components if c.endswith("_ENCODER")}
    (ok if encoders <= ALLOWED_ENCODERS else fail)(f"FFmpeg encoders: {sorted(encoders)}")
    muxers = {c[: -len("_MUXER")] for c in components if c.endswith("_MUXER")}
    (ok if muxers <= ALLOWED_MUXERS else fail)(f"FFmpeg muxers: {sorted(muxers)}")
    protocols = sorted(c[: -len("_PROTOCOL")].lower() for c in components if c.endswith("_PROTOCOL"))
    external_protocols = [p for p in protocols if p.startswith("lib")]
    (fail if external_protocols else ok)(f"FFmpeg protocols (all built in, none from an external library): {protocols}")
    return {"licence": licence, "configuration": configuration, "protocols": protocols, "encoders": sorted(encoders),
            "muxers": sorted(muxers), "external_libraries": sorted(libs)}


def check_mpv_config(info):
    config_h = (info / "mpv" / "config.h").read_text()
    (ok if re.search(r"^#define HAVE_GPL 0$", config_h, re.M) else fail)("mpv HAVE_GPL is 0")
    configuration = re.search(r'#define CONFIGURATION "([^"]*)"', config_h).group(1)
    (ok if "-Dgpl=false" in configuration else fail)("mpv configured with -Dgpl=false")
    for feature in ("D3D11", "SHADERC", "SPIRV_CROSS", "D3D_HWACCEL", "WASAPI", "UCHARDET", "ICONV", "LCMS2"):
        (ok if re.search(rf"^#define HAVE_{feature} 1$", config_h, re.M) else fail)(f"mpv feature {feature}")
    return {"configuration": configuration}


def assemble(prefix, out):
    if out.exists():
        shutil.rmtree(out)
    (out / "bin").mkdir(parents=True)
    for dll in sorted((prefix / "bin").glob("*.dll")):
        shutil.copyfile(dll, out / "bin" / dll.name)
    shutil.copytree(prefix / "include" / "mpv", out / "include" / "mpv")
    (out / "lib").mkdir()
    shutil.copyfile(prefix / "lib" / "mpv.lib", out / "lib" / "mpv.lib")
    dlls = sorted(p.name for p in (out / "bin").glob("*.dll"))
    mpv = [d for d in dlls if re.fullmatch(r"(lib)?mpv-2\.dll", d)]
    (ok if mpv else fail)(f"DLL set: {dlls}")
    return dlls


def check_imports(out, env):
    ours = {p.name.lower() for p in (out / "bin").glob("*.dll")}
    table = {}
    forbidden = 0
    for dll in sorted((out / "bin").glob("*.dll")):
        text = subprocess.run(["llvm-readobj", "--coff-imports", str(dll)], capture_output=True, text=True, check=True, env=env).stdout
        names = sorted(set(re.findall(r"^\s*Name: (\S+\.dll)\s*$", text, re.M | re.I)), key=str.lower)
        table[dll.name] = names
        # Every DLL must use the shared Universal CRT; a statically linked CRT would mean one heap per DLL.
        if not any(n.lower().startswith(("api-ms-win-crt-", "ucrtbase", "vcruntime140")) for n in names):
            fail(f"{dll.name} does not import the shared C runtime")
        for name in names:
            if FORBIDDEN_IMPORT.match(name):
                forbidden += 1
                fail(f"{dll.name} imports {name} (MinGW/MSYS runtime)")
            elif name.lower() not in ours and not SYSTEM_IMPORT.match(name):
                print(f"[check] note: {dll.name} imports {name}, neither ours nor on the system list", flush=True)
    if not forbidden:
        ok("no DLL imports a MinGW or MSYS runtime")
    ok(f"imports of {len(table)} DLLs written to imports.txt")
    lines = [f"{dll}: {', '.join(names)}" for dll, names in table.items()]
    (out / "imports.txt").write_text("\n".join(lines) + "\n")
    return table


def run_probe(prefix, out, work, env):
    bld = work / "bld" / "probe"
    bld.mkdir(parents=True, exist_ok=True)
    exe = bld / "probe.exe"
    libs = ["mpv.lib", "avcodec.lib", "avformat.lib", "avfilter.lib", "avutil.lib", "swresample.lib", "swscale.lib"]
    subprocess.run(["clang-cl", "/nologo", "/MD", "/O1", f"/I{prefix / 'include'}", str(HERE / "probe.c"), f"/Fo{bld}\\",
                    f"/Fe{exe}", "/link", f"/LIBPATH:{prefix / 'lib'}", *libs], check=True, env=env)
    run_env = dict(env, PATH=str(out / "bin") + os.pathsep + env["PATH"])
    res = subprocess.run([str(exe)], capture_output=True, text=True, env=run_env, timeout=120)
    print(res.stdout, flush=True)
    if res.returncode != 0:
        print(res.stderr, flush=True)
        fail(f"probe exited with {res.returncode}")
    text = res.stdout
    for lib, licence in re.findall(r"^licence (\w+): (.*)$", text, re.M):
        (ok if licence.strip() == LGPL else fail)(f"{lib} reports: {licence.strip()}")
    if len(re.findall(r"^licence ", text, re.M)) != 6:
        fail("probe did not report all six FFmpeg libraries")
    rejected = re.findall(r"^option (\S+): (?!success).*$", text, re.M)
    (fail if rejected else ok)(f"mpv options rejected: {rejected}")
    configuration = re.search(r"^property mpv-configuration: (.*)$", text, re.M)
    (ok if configuration and "-Dgpl=false" in configuration.group(1) else fail)("mpv-configuration at run time has -Dgpl=false")
    decoders = set(re.findall(r"^decoder-list: (.*)$", text, re.M))
    missing = sorted(REQUIRED_DECODERS - decoders)
    (fail if missing else ok)(f"required decoders present (missing: {missing}); {len(decoders)} codecs in total")
    subtitles = set(re.findall(r"^subtitle-decoder: (\S+) found\b", text, re.M))
    missing = sorted(REQUIRED_SUBTITLE_DECODERS - subtitles)
    (fail if missing else ok)(f"required subtitle decoders in libavcodec (missing: {missing})")
    demuxers = set(re.findall(r"^demuxer-lavf-list: (.*)$", text, re.M))
    missing = sorted(REQUIRED_DEMUXERS - demuxers)
    (fail if missing else ok)(f"required demuxers present (missing: {missing}); {len(demuxers)} in total")
    (out / "probe.txt").write_text(text)
    versions = {k: v for k, v in re.findall(r"^property (mpv-version|ffmpeg-version|libass-version): (.*)$", text, re.M)}
    return versions


def components(sources, out):
    rows = []
    for s in sources:
        rows.append({
            "name": s["name"],
            "version": s["version"],
            "licence": s["licence"],
            "shipped": s.get("shipped", True),
            "source": s["url"] if "url" in s else f"{s['git']} ({s.get('tag', 'commit')})",
            "checksum": f"sha256:{s['sha256']}" if "sha256" in s else f"git commit {s['commit']}",
            "note": s.get("note", ""),
        })
    (out / "components.json").write_text(json.dumps(rows, indent=2) + "\n")
    md = ["# Components of the Windows libmpv build", "",
          "Generated by build/mpv-windows/check.py from build/mpv-windows/sources.toml. Licence texts are in `licences/`.", ""]
    for shipped, title in ((True, "In the DLL set"), (False, "Build tools and generators (not shipped)")):
        md += [f"## {title}", "", "| Name | Version | Licence | Source | Checksum | Note |", "|---|---|---|---|---|---|"]
        md += [f"| {r['name']} | {r['version']} | {r['licence']} | {r['source']} | `{r['checksum']}` | {r['note']} |"
               for r in rows if r["shipped"] == shipped]
        md.append("")
    (out / "components.md").write_text("\n".join(md))


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    p.add_argument("--work", type=Path, default=Path(r"C:\mpvb"))
    p.add_argument("--out", type=Path, default=None, help="output directory (default <work>/out)")
    args = p.parse_args()
    work = args.work.resolve()
    out = (args.out or work / "out").resolve()
    prefix = work / "prefix"
    info = prefix / "share" / "current-build"
    with open(HERE / "sources.toml", "rb") as f:
        sources = tomllib.load(f)["source"]

    env = msvc_env() if os.name == "nt" else dict(os.environ)
    llvm = Path(os.environ.get("LLVM_DIR", r"C:\Program Files\LLVM")) / "bin"
    env["PATH"] = str(llvm) + os.pathsep + env["PATH"]

    ffmpeg = check_ffmpeg_config(info)
    mpv = check_mpv_config(info)
    dlls = assemble(prefix, out)
    imports = check_imports(out, env)
    versions = run_probe(prefix, out, work, env)

    shutil.copytree(info / "licences", out / "licences")
    shutil.copyfile(info / "licences" / "ffmpeg" / "COPYING.LGPLv2.1", out / "licences" / "LGPL-2.1.txt")
    components(sources, out)
    tools = {
        "clang-cl": version_of(["clang-cl", "--version"], env),
        "msvc": env.get("VCTOOLSVERSION", "unknown"),
        "windows-sdk": env.get("WINDOWSSDKVERSION", "unknown").strip("\\"),
        "meson": version_of(["meson", "--version"], env),
        "ninja": version_of(["ninja", "--version"], env),
        "cmake": version_of(["cmake", "--version"], env),
        "python": sys.version.split()[0],
        "msys2-bash": version_of([str(MSYS2_ROOT / "usr" / "bin" / "bash.exe"), "--version"], env),
        "msys2-make": version_of([str(MSYS2_ROOT / "usr" / "bin" / "make.exe"), "--version"], env),
    }
    timings = info / "timings.json"
    build_info = {"ffmpeg": ffmpeg, "mpv": mpv, "versions": versions, "dlls": dlls, "imports": imports, "tools": tools,
                  "stage_seconds": json.loads(timings.read_text()) if timings.exists() else {}}
    (out / "build-info.json").write_text(json.dumps(build_info, indent=2) + "\n")

    if failures:
        sys.exit(f"[check] {len(failures)} check(s) failed:\n  " + "\n  ".join(failures))
    print(f"[check] all checks passed; output in {out}")


if __name__ == "__main__":
    main()
