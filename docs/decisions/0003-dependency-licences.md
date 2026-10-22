# 0003: Dependency licences

Status: accepted (2026-10-22).

## Decision
- **Allowed in a distributed build:** permissively licensed code (MIT, BSD, ISC, Apache-2.0, zlib, Unicode and the like) and LGPL-2.1
  libraries. **No GPL or AGPL** in a distributed build. Also allowed: **CDLA-Permissive-2.0** (owner, 2026-10-22), a permissive data
  licence with no copyleft, first met through `webpki-root-certs` (Mozilla's root certificates) via `rustls-platform-verifier`
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
- **Exceptions:**

| Library | Licence | Decided | Scope |
|---|---|---|---|
| UniFFI (`uniffi` and its crates), `uniffi-bindgen-cs` | MPL-2.0 | 2026-10-22, owner | Used unmodified. MPL's obligations attach to its own files: the licences screen names it and where its source is; if we ever change one of its files, that file's source is published. Our own code is not affected |

- **Platform runtimes** (owner, 2026-10-22): a platform's own runtime that the app must ship to run is allowed under its vendor's
  terms, unmodified, with its licence file shipped next to it (in the Windows app's `licences\` folder). The list of every shipped file
  is in `docs/notes/windows-shell.md`.

| Runtime | Terms | Decided | Conditions |
|---|---|---|---|
| Windows App SDK 2.5.1 (WinUI and the other files its NuGet package places in a self-contained app) | Microsoft Software License Terms, Microsoft Windows App SDK (`license.txt` in the `Microsoft.WindowsAppSDK` 2.5.1 package) | 2026-10-22, owner | Below |
| .NET 10 runtime (self-contained) | MIT, with its third-party notices (`LICENSE.TXT`, `THIRD-PARTY-NOTICES.TXT` in `Microsoft.NETCore.App.Runtime.win-x64`) | already allowed (MIT) | Ship both files |
| Inno Setup 7.1.0 (its uninstaller in every install; the installer itself) | Inno Setup License: "Permission is granted to anyone to use this software for any purpose, including commercial applications" if copyright notices are kept, the origin is not misrepresented and modified versions are marked (`license.txt`, tag `is-7_1_0`, byte-identical to 6.7.3's) | 2026-10-22, owner (ADR 0009) | Ship `license.txt`; unmodified. The authors *request*, not require, a purchased licence from commercial users: "A for-profit organization with annual revenue exceeding USD 5000"; "It is not strictly required"; "regardless of the version being used" (jrsoftware.org/isorder.php, read 2026-10-22). Open before first distribution (`docs/notes/windows-shell.md`) |

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

  Accepted by the owner (2026-10-22) on these conditions, all due **before the first build goes to anyone else, testers included**:
  end-user terms that protect Microsoft per 3(b)(ii), shown by the installer; the indemnity in 3(b)(iii) is the owner's to accept;
  and the SDK's telemetry options are checked. Code signing is due by then too (ADR 0009). Until then builds exist only in CI.
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
