use std::{env, path::PathBuf, process::Command};

fn main() {
    #[cfg(target_os = "macos")]
    {
        let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set by Cargo"));
        let object = out_dir.join("macos_avfoundation.o");
        let library = out_dir.join("libcamlib_avfoundation.a");

        println!("cargo:rerun-if-changed=src/macos_avfoundation.m");

        let clang_status = Command::new("clang")
            .args([
                "-fobjc-arc",
                "-ObjC",
                "-c",
                "src/macos_avfoundation.m",
                "-o",
            ])
            .arg(&object)
            .status()
            .expect("failed to run clang");
        assert!(clang_status.success(), "clang failed");

        let ar_status = Command::new("ar")
            .arg("crus")
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
}
