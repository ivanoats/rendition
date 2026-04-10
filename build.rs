fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").ok();
    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").ok();

    if let Ok(lib_dir) = std::env::var("VIPS_LIB_DIR") {
        println!("cargo:rustc-link-search=native={}", lib_dir);
    } else if target_os.as_deref() == Some("macos") && target_arch.as_deref() == Some("aarch64") {
        println!("cargo:rustc-link-search=native=/opt/homebrew/lib");
    }
    println!("cargo:rustc-link-lib=vips");
}
