//! On a Mac, ONNX Runtime's ready-made library (for the stem splitter) calls
//! the compiler's own helper for `@available` checks, which rustc doesn't
//! link by itself: link clang's runtime library for it.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let mac = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos");
    if !mac || std::env::var_os("CARGO_FEATURE_STEMS").is_none() {
        return;
    }
    let out = std::process::Command::new("xcrun").args(["clang", "-print-resource-dir"]).output();
    let Ok(out) = out else { return };
    let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !dir.is_empty() {
        println!("cargo:rustc-link-search=native={dir}/lib/darwin");
        println!("cargo:rustc-link-lib=static=clang_rt.osx");
    }
}
