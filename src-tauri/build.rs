fn main() {
    // iPhones and cameras over USB go through Apple's ImageCaptureCore, an
    // Objective-C framework: a small bridge (src/camera_macos.m) gives Rust
    // a few plain C functions.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rerun-if-changed=src/camera_macos.m");
        cc::Build::new()
            .file("src/camera_macos.m")
            .flag("-fobjc-arc")
            .flag("-fmodules")
            .flag("-mmacosx-version-min=11.0")
            .warnings(true)
            .compile("camera_macos");
        println!("cargo:rustc-link-lib=framework=ImageCaptureCore");
        println!("cargo:rustc-link-lib=framework=Foundation");
    }
    tauri_build::build()
}
