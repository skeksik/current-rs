#!/usr/bin/env python3
"""Builds libmpv and its dependencies for Windows x64 with LLVM clang (MSVC ABI), in an LGPL-2.1 configuration.

Every source is pinned in sources.toml and verified before use. Each stage installs into <work>/prefix and leaves a stamp there, so a
rerun skips finished stages; the CI cache key covers sources.toml and these scripts, so a changed recipe never reuses an old prefix.
Python standard library only. See docs/notes/windows-mpv-build.md.

Usage (in a Windows shell; Visual Studio with the C++ workload, LLVM, CMake, Python 3.11+, meson and ninja installed, MSYS2 at
C:\\msys64 with bash and make):  python build/mpv-windows/build.py --work C:\\mpvb
"""

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tarfile
import time
import tomllib
import urllib.request
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
MSYS2_ROOT = Path(os.environ.get("MSYS2_ROOT", r"C:\msys64"))


def log(msg):
    print(f"[build] {msg}", flush=True)


def load_sources():
    with open(HERE / "sources.toml", "rb") as f:
        return {s["name"]: s for s in tomllib.load(f)["source"]}


def sha256_of(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def run(cmd, cwd=None, env=None):
    log("$ " + " ".join(str(c) for c in cmd))
    subprocess.run([str(c) for c in cmd], cwd=cwd, env=env, check=True)


class Build:
    def __init__(self, work):
        self.work = work
        self.dl = work / "dl"
        self.src = work / "src"
        self.bld = work / "bld"
        self.prefix = work / "prefix"
        self.tools = work / "tools"
        self.info = self.prefix / "share" / "current-build"
        self.stamps = self.info / "stamps"
        self.sources = load_sources()
        self.env = None
        self.timings = {}

    # ---- sources ----

    def source_dir(self, name):
        s = self.sources[name]
        return self.src / s.get("into", name)

    def fetch(self, name):
        s = self.sources[name]
        dest = self.source_dir(name)
        marker = dest / ".current-source"
        pin = s.get("sha256") or s["commit"]
        if marker.exists() and marker.read_text().strip() == pin:
            return dest
        if "url" in s:
            self.fetch_tarball(s, dest)
        else:
            self.fetch_git(s, dest)
        marker.write_text(pin + "\n")
        return dest

    def fetch_tarball(self, s, dest):
        self.dl.mkdir(parents=True, exist_ok=True)
        archive = self.dl / s["url"].rsplit("/", 1)[1]
        if not archive.exists() or sha256_of(archive) != s["sha256"]:
            log(f"download {s['url']}")
            tmp = archive.with_suffix(archive.suffix + ".part")
            with urllib.request.urlopen(s["url"], timeout=300) as r, open(tmp, "wb") as f:
                shutil.copyfileobj(r, f)
            tmp.replace(archive)
        got = sha256_of(archive)
        if got != s["sha256"]:
            sys.exit(f"{s['name']}: SHA-256 mismatch for {archive.name}: expected {s['sha256']}, got {got}")
        log(f"{s['name']} {s['version']}: SHA-256 verified")
        if dest.exists():
            shutil.rmtree(dest)
        staging = dest.with_name(dest.name + ".extract")
        if staging.exists():
            shutil.rmtree(staging)
        staging.mkdir(parents=True)
        if archive.suffix == ".zip":
            with zipfile.ZipFile(archive) as z:
                z.extractall(staging)
        else:
            with tarfile.open(archive) as t:
                t.extractall(staging, filter="data")
        entries = list(staging.iterdir())
        top = entries[0] if len(entries) == 1 and entries[0].is_dir() else staging
        dest.parent.mkdir(parents=True, exist_ok=True)
        top.rename(dest)
        if staging.exists():
            shutil.rmtree(staging)

    def fetch_git(self, s, dest):
        commit = s["commit"]
        if "tag" in s:
            out = subprocess.run(["git", "ls-remote", s["git"], f"refs/tags/{s['tag']}", f"refs/tags/{s['tag']}^{{}}"],
                                 check=True, capture_output=True, text=True).stdout.split()
            # The peeled entry (^{}) comes last when the tag is annotated.
            if not out or out[-2] != commit:
                sys.exit(f"{s['name']}: tag {s['tag']} does not point at the pinned commit {commit}: {out}")
        if dest.exists():
            shutil.rmtree(dest)
        dest.mkdir(parents=True)
        run(["git", "init", "-q", dest])
        run(["git", "-C", dest, "fetch", "-q", "--depth", "1", s["git"], commit])
        run(["git", "-C", dest, "-c", "advice.detachedHead=false", "checkout", "-q", "FETCH_HEAD"])
        head = subprocess.run(["git", "-C", dest, "rev-parse", "HEAD"], check=True, capture_output=True, text=True).stdout.strip()
        if head != commit:
            sys.exit(f"{s['name']}: fetched {head}, expected {commit}")
        log(f"{s['name']} {s['version']}: commit {commit} verified")

    # ---- environment ----

    def setup_env(self):
        env = dict(os.environ)
        if os.name == "nt":
            env = msvc_env()
        llvm = Path(os.environ.get("LLVM_DIR", r"C:\Program Files\LLVM")) / "bin"
        path = [self.tools / "nasm", self.tools / "pkgconf" / "bin", self.prefix / "bin", llvm]
        env["PATH"] = os.pathsep.join(str(p) for p in path) + os.pathsep + env["PATH"]
        env["INCLUDE"] = str(self.prefix / "include") + os.pathsep + env.get("INCLUDE", "")
        env["LIB"] = str(self.prefix / "lib") + os.pathsep + env.get("LIB", "")
        # clang's GNU-style driver targeting the MSVC ABI, linking with lld-link, as mpv's own Windows CI builds it and its
        # dependencies (mpv v0.41.0, .github/workflows/build.yml). FFmpeg's configure needs the cl-style driver (ffmpeg.sh).
        env["CC"] = "clang"
        env["CXX"] = "clang++"
        env["CC_LD"] = "lld-link"
        env["CXX_LD"] = "lld-link"
        env["WINDRES"] = "llvm-rc"
        env["PKG_CONFIG"] = str(self.tools / "pkgconf" / "bin" / "pkgconf.exe")
        env["PKG_CONFIG_PATH"] = self.prefix.as_posix() + "/lib/pkgconfig"
        # Python must not pick up modules from the sources it runs build scripts for, other than those they ask for.
        env.pop("PYTHONPATH", None)
        self.env = env

    # ---- build helpers ----

    def meson(self, name, options, default_library="static", prefix=None):
        src = self.source_dir(name)
        bld = self.bld / name
        if bld.exists():
            shutil.rmtree(bld)
        run(["meson", "setup", bld, src, f"--prefix={(prefix or self.prefix).as_posix()}", "--libdir=lib", "--buildtype=release",
             f"--default-library={default_library}", "-Db_vscrt=md", "--wrap-mode=nodownload", *options], env=self.env)
        run(["meson", "install", "-C", bld], env=self.env)

    def cmake(self, name, options, src=None):
        src = src or self.source_dir(name)
        bld = self.bld / name
        if bld.exists():
            shutil.rmtree(bld)
        run(["cmake", "-S", src, "-B", bld, "-G", "Ninja", "-DCMAKE_BUILD_TYPE=Release",
             "-DCMAKE_C_COMPILER=clang", "-DCMAKE_CXX_COMPILER=clang++", "-DCMAKE_RC_COMPILER=llvm-rc",
             "-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreadedDLL", "-DCMAKE_POLICY_VERSION_MINIMUM=3.5",
             f"-DCMAKE_INSTALL_PREFIX={self.prefix.as_posix()}", "-DCMAKE_INSTALL_LIBDIR=lib", *options], env=self.env)
        run(["cmake", "--build", bld], env=self.env)
        run(["cmake", "--install", bld], env=self.env)

    def alias_lib(self, have, want):
        """FFmpeg's MSVC toolchain links -lfoo as foo.lib; CMake and meson name some libraries differently."""
        lib = self.prefix / "lib"
        if not (lib / want).exists():
            shutil.copyfile(lib / have, lib / want)

    def stage(self, name, fn):
        stamp = self.stamps / name
        if stamp.exists():
            log(f"{name}: already built")
            return
        log(f"==== {name}")
        t0 = time.monotonic()
        fn()
        self.timings[name] = round(time.monotonic() - t0)
        self.stamps.mkdir(parents=True, exist_ok=True)
        stamp.write_text("ok\n")
        timings = self.info / "timings.json"
        old = json.loads(timings.read_text()) if timings.exists() else {}
        old.update(self.timings)
        timings.write_text(json.dumps(old, indent=2) + "\n")

    # ---- stages ----

    def nasm(self):
        src = self.fetch("nasm")
        dest = self.tools / "nasm"
        if dest.exists():
            shutil.rmtree(dest)
        shutil.copytree(src, dest)

    def pkgconf(self):
        self.fetch("pkgconf")
        self.meson("pkgconf", [], prefix=self.tools / "pkgconf")

    def zlib(self):
        self.fetch("zlib")
        self.cmake("zlib", ["-DZLIB_BUILD_SHARED=OFF", "-DZLIB_BUILD_STATIC=ON", "-DZLIB_BUILD_TESTING=OFF"])
        self.alias_lib("zs.lib", "z.lib")

    def win_iconv(self):
        self.fetch("win-iconv")
        self.cmake("win-iconv", ["-DWIN_ICONV_BUILD_SHARED=OFF", "-DWIN_ICONV_BUILD_STATIC=ON", "-DWIN_ICONV_BUILD_EXECUTABLE=OFF",
                                 "-DWIN_ICONV_BUILD_TEST=OFF"])
        self.alias_lib("libiconv.lib", "iconv.lib")

    def uchardet(self):
        self.fetch("uchardet")
        self.cmake("uchardet", ["-DBUILD_SHARED_LIBS=ON", "-DBUILD_STATIC=OFF", "-DBUILD_BINARY=OFF"])
        # The .pc file asks for libstdc++, which does not exist with the MSVC runtime.
        pc = self.prefix / "lib" / "pkgconfig" / "uchardet.pc"
        pc.write_text("".join(l for l in pc.read_text().splitlines(True) if not l.startswith("Libs.private")))

    def fribidi(self):
        self.fetch("fribidi")
        self.meson("fribidi", ["-Ddocs=false", "-Dtests=false", "-Dbin=false"], default_library="shared")

    def freetype(self):
        self.fetch("freetype")
        self.meson("freetype", ["-Dzlib=internal", "-Dbzip2=disabled", "-Dpng=disabled", "-Dharfbuzz=disabled", "-Dbrotli=disabled",
                                "-Dtests=disabled"])

    def harfbuzz(self):
        self.fetch("harfbuzz")
        # harfbuzz turns some warnings into errors through pragmas in hb.hh; clang 20 on Windows trips one
        # (-Wunused-function in hb-ot-layout-gsubgpos.hh), so keep its warnings as warnings.
        self.meson("harfbuzz", ["-Dcpp_args=-DHB_NO_PRAGMA_GCC_DIAGNOSTIC_ERROR", "--auto-features=disabled", "-Dfreetype=enabled", "-Draster=disabled", "-Dvector=disabled",
                                "-Dgpu=disabled", "-Dsubset=disabled", "-Dutilities=disabled", "-Dtests=disabled", "-Ddocs=disabled"])

    def libass(self):
        self.fetch("libass")
        self.meson("libass", ["-Dfontconfig=disabled", "-Ddirectwrite=enabled", "-Dcoretext=disabled", "-Dasm=enabled",
                              "-Dlibunibreak=disabled", "-Dtest=disabled", "-Dcompare=disabled", "-Dprofile=disabled",
                              "-Dfuzz=disabled", "-Dcheckasm=disabled"])

    def lcms2(self):
        self.fetch("lcms2")
        self.meson("lcms2", ["-Dtests=disabled", "-Djpeg=disabled", "-Dtiff=disabled", "-Dutils=false"])

    def dav1d(self):
        self.fetch("dav1d")
        self.meson("dav1d", ["-Denable_tools=false", "-Denable_tests=false", "-Denable_examples=false", "-Denable_docs=false"])
        self.alias_lib("libdav1d.a", "dav1d.lib")

    def spirv_cross(self):
        self.fetch("SPIRV-Cross")
        self.cmake("SPIRV-Cross", ["-DSPIRV_CROSS_SHARED=ON", "-DSPIRV_CROSS_STATIC=OFF", "-DSPIRV_CROSS_CLI=OFF",
                                   "-DSPIRV_CROSS_ENABLE_TESTS=OFF"])

    def shaderc(self):
        for name in ("shaderc", "glslang", "SPIRV-Tools", "SPIRV-Headers"):
            self.fetch(name)
        self.cmake("shaderc", ["-DSHADERC_SKIP_TESTS=ON", "-DSHADERC_SKIP_EXAMPLES=ON", "-DSHADERC_SKIP_EXECUTABLES=ON",
                               "-DSHADERC_SKIP_COPYRIGHT_CHECK=ON", "-DSHADERC_ENABLE_SHARED_CRT=ON",
                               "-DSHADERC_ENABLE_WERROR_COMPILE=OFF", "-DSPIRV_SKIP_EXECUTABLES=ON", "-DSPIRV_SKIP_TESTS=ON",
                               "-DSPIRV_WERROR=OFF", "-DSPIRV_HEADERS_SKIP_EXAMPLES=ON", "-DENABLE_GLSLANG_BINARIES=OFF",
                               "-DGLSLANG_TESTS=OFF", "-DSKIP_GLSLANG_INSTALL=ON", "-DSKIP_SPIRV_TOOLS_INSTALL=ON"])

    def libplacebo(self):
        for name in ("libplacebo", "fast_float", "Vulkan-Headers", "jinja", "markupsafe"):
            self.fetch(name)
        self.meson("libplacebo", ["-Dvulkan=disabled", "-Dopengl=disabled", "-Dd3d11=enabled", "-Dshaderc=enabled",
                                  "-Dglslang=disabled", "-Dlcms=enabled", "-Ddovi=enabled", "-Dlibdovi=disabled",
                                  "-Dxxhash=disabled", "-Dunwind=disabled", "-Ddemos=false", "-Dtests=false", "-Dbench=false",
                                  "-Dfuzz=false"], default_library="shared")

    def ffmpeg(self):
        src = self.fetch("ffmpeg")
        bld = self.bld / "ffmpeg"
        if bld.exists():
            shutil.rmtree(bld)
        bld.mkdir(parents=True)
        bash = MSYS2_ROOT / "usr" / "bin" / "bash.exe"
        env = dict(self.env, MSYSTEM="MSYS", CHERE_INVOKING="1", MSYS2_PATH_TYPE="inherit")
        run([bash, "--noprofile", "--norc", (HERE / "ffmpeg.sh").as_posix(), src.as_posix(), self.prefix.as_posix(),
             self.env["PKG_CONFIG"].replace("\\", "/")], cwd=bld, env=env)
        keep = self.info / "ffmpeg"
        keep.mkdir(parents=True, exist_ok=True)
        for f in ("config.h", "config_components.h", "ffbuild/config.mak", "ffbuild/config.log"):
            shutil.copyfile(bld / f, keep / Path(f).name)

    def mpv(self):
        self.fetch("mpv")
        enabled = ["d3d11", "shaderc", "spirv-cross", "d3d-hwaccel", "wasapi", "win32-threads", "iconv", "uchardet", "lcms2", "zlib"]
        self.meson("mpv", ["--auto-features=disabled", "-Dprefer_static=true", "-Dgpl=false", "-Dcplayer=false", "-Dlibmpv=true",
                           "-Dtests=false", "-Dbuild-date=false", "-Dgl=disabled", "-Dlua=disabled", "-Djavascript=disabled",
                           *(f"-D{f}=enabled" for f in enabled)], default_library="shared")
        keep = self.info / "mpv"
        keep.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(self.bld / "mpv" / "config.h", keep / "config.h")
        shutil.copyfile(self.bld / "mpv" / "meson-logs" / "meson-log.txt", keep / "meson-log.txt")

    def licences(self):
        """Copies each component's licence files next to the build, so the check step needs only the prefix."""
        dest = self.info / "licences"
        if dest.exists():
            shutil.rmtree(dest)
        for name, s in self.sources.items():
            for f in s.get("licence_files", []):
                out = dest / name / Path(f).name
                out.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(self.source_dir(name) / f, out)

    def all(self):
        self.setup_env()
        stages = [("nasm", self.nasm), ("pkgconf", self.pkgconf), ("zlib", self.zlib), ("win-iconv", self.win_iconv),
                  ("uchardet", self.uchardet), ("fribidi", self.fribidi), ("freetype", self.freetype), ("harfbuzz", self.harfbuzz),
                  ("libass", self.libass), ("lcms2", self.lcms2), ("dav1d", self.dav1d), ("ffmpeg", self.ffmpeg),
                  ("SPIRV-Cross", self.spirv_cross), ("shaderc", self.shaderc), ("libplacebo", self.libplacebo), ("mpv", self.mpv),
                  ("licences", self.licences)]
        for name, fn in stages:
            self.stage(name, fn)


def msvc_env():
    """The environment of vcvars64.bat from the newest Visual Studio with the C++ x64 tools, found with vswhere."""
    vswhere = Path(os.environ["ProgramFiles(x86)"]) / "Microsoft Visual Studio" / "Installer" / "vswhere.exe"
    vs = subprocess.run([str(vswhere), "-latest", "-products", "*", "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
                         "-property", "installationPath"], check=True, capture_output=True, text=True).stdout.strip()
    vcvars = Path(vs) / "VC" / "Auxiliary" / "Build" / "vcvars64.bat"
    # A string, not a list: cmd needs the quotes exactly as written.
    out = subprocess.run(f'cmd /s /c ""{vcvars}" >nul && set"', check=True, capture_output=True, text=True).stdout
    env = {}
    for line in out.splitlines():
        k, sep, v = line.partition("=")
        if sep:
            # Windows names are case-insensitive (`set` prints "Path"); one spelling avoids duplicates in the child environment.
            env[k.upper()] = v
    log(f"MSVC environment from {vcvars}")
    return env


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    p.add_argument("--work", type=Path, default=Path(r"C:\mpvb"), help="work directory (short path: Windows path limits)")
    p.add_argument("--fetch-only", action="store_true", help="download and verify every source, build nothing")
    args = p.parse_args()
    b = Build(args.work.resolve())
    if args.fetch_only:
        for name in b.sources:
            b.fetch(name)
        return
    b.all()


if __name__ == "__main__":
    main()
