//! Build-time icon/version adapter translated from resources/windows/ned.rc.in.
use std::{env, fs, path::PathBuf, process::Command};

pub fn compile() {
    let target = env::var("TARGET").expect("Cargo supplies TARGET");
    if !target.contains("windows") {
        return;
    }
    println!("cargo:rerun-if-changed=resources/icons/bed.ico");
    println!("cargo:rerun-if-changed=resources/windows/bed.rc.in");
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let icon = root
        .join("resources/icons/bed.ico")
        .to_string_lossy()
        .replace('\\', "/");
    assert!(!icon.contains('"'), "resource icon path contains a quote");
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let numeric = [
        "CARGO_PKG_VERSION_MAJOR",
        "CARGO_PKG_VERSION_MINOR",
        "CARGO_PKG_VERSION_PATCH",
    ]
    .map(|key| env::var(key).unwrap())
    .join(",")
        + ",0";
    let template = fs::read_to_string(root.join("resources/windows/bed.rc.in")).unwrap();
    let source = template
        .replace("@BED_ICON_PATH@", &icon)
        .replace("@BED_VERSION@", &version)
        .replace("@BED_NUMERIC_VERSION@", &numeric);
    let rc = out.join("bed.rc");
    fs::write(&rc, source).unwrap();
    let (mut command, object) = if target.contains("msvc") {
        let mut command =
            cc::windows_registry::find(&target, "rc.exe").unwrap_or_else(|| Command::new("rc.exe"));
        let object = out.join("bed.res");
        command
            .arg("/nologo")
            .arg("/c65001")
            .arg("/fo")
            .arg(&object)
            .arg(&rc);
        (command, object)
    } else {
        let program = env::var_os("WINDRES").unwrap_or_else(|| "windres".into());
        let mut command = Command::new(program);
        let object = out.join("bed-resource.o");
        command
            .arg("-i")
            .arg(&rc)
            .arg("-O")
            .arg("coff")
            .arg("-o")
            .arg(&object);
        (command, object)
    };
    let status = command
        .status()
        .expect("Windows SDK resource compiler is required");
    assert!(status.success(), "resource compiler failed: {status}");
    println!("cargo:rustc-link-arg-bin=bed={}", object.display());
}
