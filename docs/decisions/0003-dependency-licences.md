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
