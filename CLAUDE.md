# Current

Cross-platform media client. It browses, requests and plays content from **Jellyfin** (library + playback), **Seerr** (discover +
requests) and **Stash** (scenes via GraphQL). Solo project. Anyone may use it against their own servers: every service is optional at
runtime.

**A shared Rust core with a native UI per platform** (ADR 0001, ADR 0002): WinUI 3 in C# on Windows, SwiftUI on macOS, GTK4 +
libadwaita in Rust on Linux; Android, iOS, tvOS and the web later. **Desktop first:** Windows, then macOS, then Linux.

## Principles
- **All logic lives in the Rust core.** UIs render the core's state and send user actions; they hold only platform integration.
- **Native behaviour by default.** Each app looks and behaves as its platform's own apps do. If the platform does something a certain
  way, keep it. Departing from it needs a reason and the owner's agreement, recorded in a decision record (ADR 0001).
- **The player is our own on every platform**, styled and behaving like a native player there (ADR 0001).

## Layout
```
crates/model        current-model: domain types (pure, no I/O)
crates/net          current-net: HTTP client per server, typed errors, redacted request log, plain-HTTP rule, gateway credentials (ADR 0017)
crates/store        current-store: settings file, secrets in the OS keyring, device id (ADR 0013)
crates/jellyfin     current-jellyfin: Jellyfin client; so far server setup (ADR 0012), discovery (ADR 0016), sign-in (ADR 0015), gateway answers and header placement (ADR 0017), browse queries and images (ADR 0018)
crates/mpv          current-mpv: the libmpv engine (ADR 0019): client API declarations, event thread, options, display facts, swap chain, capabilities, log redaction; the only crate with `unsafe`; links libmpv only with its `libmpv` feature
crates/app          current-app: composition root (App), capabilities state, observable lists, add-a-server and sign-in flows, sessions, local discovery, gateways (ADR 0017), image cache (ADR 0018)
crates/ffi          current-ffi: UniFFI surface over current-app (runtime, TaskHandle, CancelToken, CoreError; ADR 0004); opening, setup, sign-in, sessions
tools/uniffi-bindgen  UniFFI's generator at the workspace's uniffi version, for Swift
tools/jellyfin-stand-in  a fake Jellyfin on 127.0.0.1 for the bridge tests (Rust, C#, Swift); never shipped
bindings/csharp     Current.Bindings: generated C# (not checked in) + public adapters, and a console test runner
bindings/swift      SwiftPM package: XCFramework + generated Swift (not checked in) + adapters, with tests
apps/windows        the WinUI 3 app (Current.Shell, builds Current.exe), its Inno Setup installer and install test (ADR 0009)
docs/               architecture.md, roadmap.md, decisions/ (ADRs)
```
Target layout (ADR 0002): `crates/{model,net,store,jellyfin,seerr,stash,playback,mpv,app,ffi}`, `apps/{windows,macos,linux}`. Each crate
is created by the task that first needs it; a new crate copies the shape of `crates/model` (workspace version, edition, lints).

## Commands
```
cargo build --workspace
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check                                       # licences and sources (deny.toml, ADR 0003); install: cargo install --locked cargo-deny
cargo test -p current-mpv --features libmpv            # the engine against a real libmpv (MPV_LIB_DIR for its import library; openssl on PATH); CI's "mpv (Windows)" job runs it on the D6 build
pwsh bindings/csharp/build.ps1; pwsh bindings/csharp/test.ps1   # Windows: build current-ffi, generate C#, build and test the adapters
pwsh apps/windows/build.ps1; pwsh apps/windows/smoke.ps1 -Exe apps/windows/out/app/Current.exe -StandIn target/x86_64-pc-windows-msvc/release/jellyfin-stand-in.exe   # Windows, after the C# build: publish and smoke-run the app (with the sign-in window on the stand-in)
pwsh apps/windows/installer/build-installer.ps1 -Iscc <path to ISCC.exe>   # Windows: build the installer (Inno Setup 7)
sh bindings/swift/build.sh; swift test --package-path bindings/swift   # macOS: build current-ffi, generate Swift, XCFramework, test
```
The C# build needs uniffi-bindgen-cs: `cargo install --locked --git https://github.com/NordSecurity/uniffi-bindgen-cs --tag
v0.11.1+v0.31.0 uniffi-bindgen-cs`.
CI (`.github/workflows/ci.yml`) runs the Rust checks on Linux, Windows and macOS, cargo-deny on Linux, the bindings jobs "Bindings
(C#, Windows)" and "Bindings (Swift, macOS)", "mpv (Windows)" (current-mpv against the D6 libmpv build), and "App (Windows)" (publish, smoke run, installer, install/upgrade/uninstall test). C# and Swift cannot be built in a Linux cloud container: say so in the
PR and let CI build them.

## Conventions
- Rust edition 2024, toolchain pinned in `rust-toolchain.toml`. `unsafe_code` is forbidden workspace-wide; a crate that must call C
  directly (`current-mpv`) may allow it, with a comment saying why. UniFFI's generated code needs no exception (ADR 0004).
- Wire types (DTOs) stay private to their service crate; public APIs use `current-model` types.
- Open values, not closed enums, for anything a server defines (services, media kinds, permissions): unknown values pass through.
- No feature may assume a service exists: read the capabilities state and hide what is unavailable.
- Errors are typed (`thiserror`); no `unwrap`/`expect` outside tests unless the invariant is local and commented.
- Tests: unit tests next to the code; test service clients against recorded or hand-written responses, never a real server in CI.
  A test that cannot run must be skipped visibly (`#[ignore]` with a reason), never pass by returning early.
- **Dependencies:** never add a dependency, tool or GitHub Action, or bump a version (Rust toolchain included), without the owner's
  approval. Every crate is declared once in the root `Cargo.toml` `[workspace.dependencies]` with an exact version (`=x.y.z`); member
  crates use `name.workspace = true`. Licences per ADR 0003, checked by `cargo deny check`. Actions are pinned by commit SHA with the
  tag in a comment.
  Standing exception (owner, 2026-10-23): on Windows, Microsoft's own platform packages (the Windows SDK, the Windows App SDK and its
  component packages, the Windows Community Toolkit) are preferred and approved in advance. Pin exact versions, check their licences
  and their dependencies under ADR 0003 (licence texts shipped with the app), and report what was added in the PR.
- **Do not assert API or platform facts without a source.** Quote the source and the date read; mark anything unchecked "not verified".
- **The repo is public.** No hostnames, IPs, keys, tokens, user names, server names or library titles in code, tests, fixtures, docs or
  commits. Example addresses only: `192.168.1.10`, `media.example.com`, `127.0.0.1`. Describe private server forks only generically.
  The owner's servers and credentials never go into a cloud prompt or environment.
- Do not name third-party apps that inspired a design in code, docs or commits ("an existing third-party client" at most).
- Never add a licences screen unprompted: the owner says when.

## Git workflow
- `main` is protected and always green: PRs only, squash merges only, never force-push.
- One task = one branch = one PR. Keep PRs small. Never reuse a branch after its PR is merged.
- Branch names are `<type>/<topic>`. Type: `feat`, `fix`, `docs`, `refactor`, `test`, `build`, `chore`, or `spike` (an experiment whose
  PR carries only its decision record or notes). Topic: a few lowercase words joined by hyphens.
- Bring main into a branch with a merge commit, never a rebase.
- Commit messages: imperative, short subject, optional body explaining why. No `Co-Authored-By`, `Claude-Session` or "Generated with
  Claude Code" lines in commits or PR descriptions.
- Author and committer of every commit: `skeksik <personal@skeksik.com>` (in a fresh environment, `git config user.name skeksik` and
  `git config user.email personal@skeksik.com` before the first commit).
- Before a PR, run the commands above and report failures honestly. Fill in `.github/pull_request_template.md`, including what was not
  verified.
- Record architectural decisions as short ADRs in `docs/decisions/` (next free number).

## Working with Claude
- Non-trivial work: propose a plan first, then implement in small steps.
- `/manage` runs a cloud session as the manager that starts worker sessions, reviews their PRs and merges after the owner's rules
  (`.claude/commands/manage.md`).
