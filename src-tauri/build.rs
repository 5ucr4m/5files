fn main() {
    // Windows: the app manifest (Common Controls v6, needed by native dialogs) is linked
    // into every binary — the app *and* the test executables. tauri-build only adds it
    // to the app, and tests then fail to start with STATUS_ENTRYPOINT_NOT_FOUND.
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    let mut attrs = tauri_build::Attributes::new();
    if msvc {
        attrs = attrs.windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
    }
    tauri_build::try_build(attrs).expect("failed to run the Tauri build script");
    if msvc {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("windows-app-manifest.xml");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    }
}
