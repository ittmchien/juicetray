use std::env;
use std::path::PathBuf;

// Embeds the Windows manifest (linker args, so a macOS host needs no tools) and, when the
// build host is Windows, the app icon resource; other targets get nothing.
fn main() {
    let packaging =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"))
            .join("../packaging/windows");
    for file in ["juicetray.manifest", "juicetray.rc", "juicetray.ico"] {
        println!("cargo:rerun-if-changed={}", packaging.join(file).display());
    }

    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let manifest = packaging.join("juicetray.manifest");
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }

    // The resource compiler exists only on a Windows host; a build script is compiled for the host.
    #[cfg(windows)]
    embed_resource::compile(packaging.join("juicetray.rc"), embed_resource::NONE)
        .manifest_optional()
        .expect("compile the Windows icon resource");
}
