use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // Build scripts run on the host, so select the backend from the target, not `cfg!`.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        build_macos();
    }
}

fn build_macos() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    let object = out_dir.join("macos_avfoundation.o");
    let library = out_dir.join("libcamlib_avfoundation.a");
    let arch = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86_64") => "x86_64",
        other => panic!("unsupported macOS architecture {other:?}"),
    };

    println!("cargo:rerun-if-changed=src/macos_avfoundation.m");

    let clang_status = Command::new("clang")
        .args([
            "-arch",
            arch,
            "-mmacosx-version-min=11.0",
            "-fobjc-arc",
            "-ObjC",
            "-O2",
            "-c",
            "src/macos_avfoundation.m",
            "-o",
        ])
        .arg(&object)
        .status()
        .expect("failed to run clang");
    assert!(clang_status.success(), "clang failed");

    let ar_status = Command::new("ar")
        .arg("crs")
        .arg(&library)
        .arg(&object)
        .status()
        .expect("failed to run ar");
    assert!(ar_status.success(), "ar failed");

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=camlib_avfoundation");
    println!("cargo:rustc-link-lib=framework=AVFoundation");
    println!("cargo:rustc-link-lib=framework=CoreMedia");
    println!("cargo:rustc-link-lib=framework=CoreVideo");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=AppKit");
}
