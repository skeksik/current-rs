# 0003: Dependency licences

Status: accepted (2026-10-22).

## Decision
- **Allowed in a distributed build:** permissively licensed code (MIT, BSD, ISC, Apache-2.0, zlib, Unicode and the like), code
  dedicated to the public domain (owner, 2026-10-22, first met as win-iconv in the Windows libmpv build) and LGPL-2.1 libraries.
  **No GPL or AGPL** in a distributed build. Also allowed: **CDLA-Permissive-2.0** (owner, 2026-10-22), a permissive data licence
  with no copyleft, first met through `webpki-root-certs` (Mozilla's root certificates) via `rustls-platform-verifier`
  (https://cdla.dev/permissive-2-0/, read 2026-10-22).
- **LGPL-3.0 or MPL-2.0 only by an explicit decision per library**, recorded here.
- **For every LGPL library we ship:** its licence and source (with our changes and build configuration) are published with each release;
  it stays replaceable (a separate dynamic library; where only static linking is possible, our app is offered as object files for
  relinking); our terms allow modifying and debugging it; and the app has an open-source licences screen. FFmpeg and libmpv are built from
  source in LGPL-2.1 configurations. mpv is LGPL-2.1+ only when built without its GPL files (`-Dgpl=false`) and linked against an
  FFmpeg built without `--enable-gpl`, `--enable-version3` and `--enable-nonfree` (mpv's `Copyright` file, read 2026-10-22,
  https://github.com/mpv-player/mpv/blob/master/Copyright). Common prebuilt Windows builds do not qualify: shinchiro's builds document no
  LGPL option, and zhongfly's "lgpl" package links FFmpeg as LGPL-3.0 (their READMEs, read 2026-10-22,
  https://github.com/shinchiro/mpv-winbuild-cmake, https://github.com/zhongfly/mpv-winbuild).
- **Our Windows libmpv build** (D6, `build/mpv-windows/`, `docs/notes/windows-mpv-build.md`) ships only LGPL-2.1-or-later and
  permissive components; its CI fails if FFmpeg reports another licence or mpv was built with GPL code. Two components needed a reading:
  - **uchardet 0.0.8** is covered by LGPL-2.1-or-later and needs no exception. Every file in `src/` of
    `uchardet-0.0.8.tar.xz` (https://www.freedesktop.org/software/uchardet/releases/, read 2026-10-22) carries Mozilla's tri-licence
    header: "Version: MPL 1.1/GPL 2.0/LGPL 2.1 [...] Alternatively, the contents of this file may be used under the terms of either the
    GNU General Public License Version 2 or later (the "GPL"), or the GNU Lesser General Public License Version 2.1 or later (the
    "LGPL") [...] If you do not delete the provisions above, a recipient may use your version of this file under the terms of any one of
    the MPL, the GPL or the LGPL." We use it under LGPL-2.1-or-later (owner's approval of uchardet, 2026-10-22).
  - **LAME** is left out (owner, 2026-10-22: a player needs no MP3 encoder).
- **Exceptions:**

| Library | Licence | Decided | Scope |
|---|---|---|---|
| UniFFI (`uniffi` and its crates), `uniffi-bindgen-cs` | MPL-2.0 | 2026-10-22, owner | Used unmodified. MPL's obligations attach to its own files: the licences screen names it and where its source is; if we ever change one of its files, that file's source is published. Our own code is not affected |
| glslang's Bison-generated parser (`glslang/MachineIndependent/glslang_tab.cpp`, via shaderc in the Windows libmpv build) | GPL-3.0-or-later WITH Bison-exception-2.2 | 2026-10-22, owner | Allowed: the exception waives the GPL for the generated file. Its text, in the file header and in glslang's `LICENSE.txt` (commit e1b562a8, read 2026-10-22): "As a special exception, you may create a larger work that contains part or all of the Bison parser skeleton and distribute that work under terms of your choice, so long as that work isn't itself a parser generator using the skeleton or a modified version thereof as a parser skeleton." The rest of glslang is BSD, MIT and Apache-2.0 |

- **Platform runtimes** (owner, 2026-10-22): a platform's own runtime that the app must ship to run is allowed under its vendor's
  terms, unmodified, with its licence file shipped next to it (in the Windows app's `licences\` folder). The list of every shipped file
  is in `docs/notes/windows-shell.md`.

| Runtime | Terms | Decided | Conditions |
|---|---|---|---|
| Windows App SDK 2.5.1 (WinUI and the other files its NuGet package places in a self-contained app) | Microsoft Software License Terms, Microsoft Windows App SDK (`license.txt` in the `Microsoft.WindowsAppSDK` 2.5.1 package) | 2026-10-22, owner | Below |
| Windows SDK projection for .NET (`Microsoft.Windows.SDK.NET.dll`, `WinRT.Runtime.dll`, from `Microsoft.Windows.SDK.NET.Ref`, which every `net10.0-windows10.x` app gets from the .NET SDK) | Microsoft Software License Terms, Microsoft Windows Software Development Kit (SDK) for Windows 10 (the package's licence URL, https://aka.ms/WinSDKLicenseURL; no licence file in the package) | 2026-10-22, owner | Below; `licences\WindowsSDK-projection.txt` points to the terms |
| WebView2 SDK (`WebView2Loader.dll`, `Microsoft.Web.WebView2.Core.dll`, `Microsoft.Web.WebView2.Core.Projection.dll`, a dependency of WinUI) | BSD-3-Clause (`LICENSE.txt` in `Microsoft.Web.WebView2` 1.0.3719.77) | already allowed (BSD) | Ship `LICENSE.txt` |
| .NET 10 runtime (self-contained) | MIT, with its third-party notices (`LICENSE.TXT`, `THIRD-PARTY-NOTICES.TXT` in `Microsoft.NETCore.App.Runtime.win-x64`) | already allowed (MIT) | Ship both files |
| Inno Setup 7.1.0 (its uninstaller in every install; the installer itself) | Inno Setup License: "Permission is granted to anyone to use this software for any purpose, including commercial applications" if copyright notices are kept, the origin is not misrepresented and modified versions are marked (`license.txt`, tag `is-7_1_0`, byte-identical to 6.7.3's) | 2026-10-22, owner (ADR 0009) | Ship `license.txt`; unmodified. The authors *request*, not require, a purchased licence from commercial users: "A for-profit organization with annual revenue exceeding USD 5000"; "It is not strictly required"; "regardless of the version being used" (jrsoftware.org/isorder.php, read 2026-10-22). Open before first distribution (`docs/notes/windows-shell.md`) |
| Visual C++ Redistributable, x64 (`vc_redist.x64.exe`, which installs `vcruntime140.dll`, `vcruntime140_1.dll` and `msvcp140.dll`; the libmpv DLLs need them, docs/notes/windows-mpv-build.md) | Microsoft Software License Terms of Visual Studio, "Distributable Code" section and its REDIST list | 2026-10-25, owner | Below |

  The Windows App SDK terms go beyond "redistributable", quoted from its `license.txt` (read 2026-10-22):
  - 3(a)(i): "Any files that are binplaced with your application by the WindowsAppSDK NuGet package are, by definition, permitted to
    be redistributed. This applies to both framework package dependent and self-contained deployments."
  - 3(b): "For any code you distribute, you must: i. add significant primary functionality to it in your applications; ii. require
    distributors and external end users to agree to terms that protect it and Microsoft at least as much as this agreement; and iii.
    indemnify, defend, and hold harmless Microsoft from any claims, including attorneys' fees, related to the distribution or use of
    your applications, except to the extent that any claim is based solely on the unmodified distributable code."
  - 3(c): no use of "Microsoft's trademarks or trade dress in your application in any way that suggests your application comes from or
    is endorsed by Microsoft", and no making the distributable code subject to a licence that requires its source to be disclosed.
  - 2(a): "The software may collect information about you and your use of the software, and send that to Microsoft. [...] You may
    opt-out of many of these scenarios, but not all, as described in the product documentation."

  The Windows SDK terms (sdk_license.rtf, read 2026-10-22) allow distributing the files on its REDIST.TXT lists. The list
  (learn.microsoft.com/legal/windows-sdk/redist, updated 2024-10-21, read 2026-10-22) has an entry "Microsoft.Windows.SDK.NET.Ref":
  "The files included in the Microsoft.Windows.SDK.NET.Ref Nuget package [...] may be distributed unmodified as a NuGet package, or as
  part of your program in order to enable your application to call WinRT Apis", naming `./lib/net8.0/Microsoft.Windows.SDK.NET.dll`
  and `./lib/net8.0//WinRT.Runtime.dll` (and net6.0); the files we ship are the same package's newer `lib` folder. Its "Distribution
  Requirements" are those of the Windows App SDK plus one: "Add significant primary functionality to it in your programs; [...]
  Require distributors and external end users to agree to terms that protect it at least as much as this agreement; [...] Display
  your valid copyright notice on your programs; and Indemnify, defend, and hold harmless Microsoft from any claims, including attorneys
  fees, related to the distribution or use of your programs." Among its restrictions: "Distribute Distributable Code to run on a
  platform other than the Microsoft operating system platform" is not allowed.

  The Visual C++ runtime (owner, 2026-10-25: Microsoft's installer, not app-local copies or a static CRT) is needed already: the core's own
  `current_ffi.dll` imports `VCRUNTIME140.dll` (Rust's MSVC target links the CRT dynamically; listed by CI's "Bindings (C#, Windows)"
  job with `llvm-readobj --coff-imports`, run 40173566806, 2026-10-25), and so do the libmpv DLLs. The app's installer runs
  Microsoft's `vc_redist.x64.exe` when `HKLM\SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64` shows the runtime missing or
  older than the toolset that built the DLLs ("We only support using a Redistributable package that's the same as or newer than your
  toolset version", "Determine which DLLs to redistribute", learn.microsoft.com, updated 2023-10-26). Central deployment lets Windows
  Update service the runtime ("Deployment in Microsoft C++", updated 2025-11-05). The REDIST list for the runner's Visual Studio
  ("Visual Studio Redistribution", learn.microsoft.com/visualstudio/releases/2026/redistribution, updated 2025-11-11, read
  2026-10-25), section "Visual C++ Runtime Files": "Subject to the License Terms for the software, you may copy and distribute with your
  program any of the files within the following folder and its subfolders except as noted below. You may not modify these files.
  [VisualStudioFolder]\VC\redist", and "Distribution of the Visual C++ Runtime Redistributable package, merge modules, and individual
  binaries obtained from those pages is limited to licensed Visual Studio users and is subject to its license terms"; the
  `debug_nonredist` folders may not be distributed. The Visual Studio licence's own "Distributable Code" conditions (what they require
  of the distributor, and whether the edition the release build uses counts as licensed for this) were **not read: not verified**,
  and are due before the first build goes to anyone else, with the conditions below. Packaging is the installer task's (roadmap). The
  CI install test cannot notice a missing runtime: the runner image has Visual Studio, hence the runtime, installed.

  Accepted by the owner (2026-10-22) on these conditions, all due **before the first build goes to anyone else, testers included**:
  end-user terms that protect Microsoft per 3(b)(ii) (and the Windows SDK's equivalent), shown by the installer; the indemnities of
  both are the owner's to accept; the SDK's telemetry options are checked; and the app shows a copyright notice (Settings shows
  "© 2026 Skeksik" next to the version). Code signing is due by then too (ADR 0009). Until then builds exist only in CI.
- **Current's own code is proprietary.** No open-source licence is granted and the repo has no LICENSE file. A licence granted on public
  code cannot be taken back for copies already made, so none is added while the repo is public.
- **Checked by machine.** `cargo-deny` runs in CI against `deny.toml`, which lists the allowed licences and the exceptions above (each
  exception enters `deny.toml`, crate by crate, in the PR that adds the library). A new licence or exception changes `deny.toml` and this
  record in the same PR, with the owner's approval. .NET (NuGet) and Swift packages get
  the same check when their first package is added.
- **Patents** are a separate check per codec before a commercial release: prefer OS and hardware decoders.

## Consequences
- Build tools that are not shipped (cargo-deny, test-only crates) still need the owner's approval to be added, but not a licence exception.
- The licences screen is added when the owner says so; until then, the obligations above are tracked in this record and `deny.toml`.
