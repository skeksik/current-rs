//! Links libmpv when the `libmpv` feature is on: `mpv.lib` (Windows, from the D6 build's `lib/`) or `libmpv` (Linux, the system's).
//! `MPV_LIB_DIR` adds a search directory, for a libmpv that is not on the linker's default path.

fn main() {
    println!("cargo:rerun-if-env-changed=MPV_LIB_DIR");
    if std::env::var_os("CARGO_FEATURE_LIBMPV").is_none() {
        return;
    }
    if let Some(dir) = std::env::var_os("MPV_LIB_DIR") {
        println!(
            "cargo:rustc-link-search=native={}",
            std::path::Path::new(&dir).display()
        );
    }
    println!("cargo:rustc-link-lib=dylib=mpv");
}
