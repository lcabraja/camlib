use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // Build scripts run on the host, so select the backend from the target, not `cfg!`.
    match env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("macos") => build_macos(),
        Ok("linux") => build_linux(),
        _ => {}
    }
}

fn archive(out_dir: &Path, object: &Path, name: &str) {
    let library = out_dir.join(format!("lib{name}.a"));
    let _ = std::fs::remove_file(&library);
    let ar = env::var("AR").unwrap_or_else(|_| "ar".into());
    let ar_status = Command::new(ar)
        .arg("crs")
        .arg(&library)
        .arg(object)
        .status()
        .expect("failed to run ar");
    assert!(ar_status.success(), "ar failed");
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static={name}");
}

fn build_linux() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    let object = out_dir.join("linux_v4l2.o");
    println!("cargo:rerun-if-changed=src/linux_v4l2.c");
    println!("cargo:rerun-if-env-changed=CC");
    let cc = env::var("CC").unwrap_or_else(|_| "cc".into());
    let status = Command::new(cc)
        .args([
            "-std=c11",
            "-O2",
            "-fPIC",
            "-Wall",
            "-c",
            "src/linux_v4l2.c",
            "-o",
        ])
        .arg(&object)
        .status()
        .expect("failed to run the C compiler (set CC to override)");
    assert!(status.success(), "compiling src/linux_v4l2.c failed");
    archive(&out_dir, &object, "camlib_v4l2");
}

fn build_macos() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    let object = out_dir.join("macos_avfoundation.o");
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

    archive(&out_dir, &object, "camlib_avfoundation");
    println!("cargo:rustc-link-lib=framework=AVFoundation");
    println!("cargo:rustc-link-lib=framework=CoreMedia");
    println!("cargo:rustc-link-lib=framework=CoreVideo");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=AppKit");
}
