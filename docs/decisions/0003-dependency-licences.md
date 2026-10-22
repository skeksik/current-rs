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
