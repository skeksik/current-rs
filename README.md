# Current

A cross-platform client for browsing, requesting and playing media from [Jellyfin](https://jellyfin.org),
[Seerr](https://github.com/seerr-team/seerr) and [Stash](https://github.com/stashapp/stash), built on a shared Rust core.

## Requirements
- Rust through [rustup](https://rustup.rs); the exact toolchain is pinned in `rust-toolchain.toml` and installed on first use.

## Commands
```
cargo build --workspace
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

## Copyright
Copyright (c) 2026 Skeksik. All rights reserved.
