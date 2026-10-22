//! `cargo run -p uniffi-bindgen -- generate --library <lib> --language swift --out-dir <dir>`. Built in the workspace so the
//! generator is always the `uniffi` version current-ffi links (ADR 0004). C# comes from uniffi-bindgen-cs instead.

fn main() {
    uniffi::uniffi_bindgen_main();
}
