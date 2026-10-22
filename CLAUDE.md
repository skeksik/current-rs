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
- **Research from the Kotlin project** (`docs/reference/kmp/`) is reference, not a rule. Re-check facts before relying on them; its UI
  rules apply only when a decision record here adopts them.

## Layout
```
crates/model        current-model: domain types (pure, no I/O)
docs/               architecture.md, roadmap.md, decisions/ (ADRs), reference/kmp/ (the Kotlin project's research)
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
```
CI (`.github/workflows/ci.yml`) runs the Rust checks on Linux, Windows and macOS, and cargo-deny on Linux. C# and Swift builds join CI
with the bridge spike (D1). They cannot be built in a Linux cloud container: say so in the PR and let CI build them.

## Conventions
- Rust edition 2024, toolchain pinned in `rust-toolchain.toml`. `unsafe_code` is forbidden workspace-wide; a crate that must call C
  (`current-mpv`, `current-ffi`) may allow it, with a comment saying why.
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
