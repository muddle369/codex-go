use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=app-icon.rc");
    println!("cargo:rerun-if-changed=../../assets/images/codex-go.ico");
    println!("cargo:rerun-if-changed=../codexx-manager/src-tauri/windows-app-manifest.xml");

    let target = env::var("TARGET").unwrap_or_default();
    if !target.contains("windows") {
        return;
    }

    if target.contains("msvc") {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("../../assets/images/codex-go.ico");
        resource.set_manifest(include_str!(
            "../codexx-manager/src-tauri/windows-app-manifest.xml"
        ));
        resource.compile().expect("compile launcher icon resource");
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let windres = if target == "x86_64-pc-windows-gnu" {
        "x86_64-w64-mingw32-windres".to_string()
    } else {
        format!("{target}-windres")
    };
    let resource = out_dir.join("codexgo-resource.o");
    let status = Command::new(windres)
        .current_dir(&manifest_dir)
        .arg("app-icon.rc")
        .arg(&resource)
        .status()
        .expect("run windres for launcher resources");
    assert!(status.success(), "windres failed for launcher resources");

    println!("cargo:rustc-link-arg={}", resource.display());
}
