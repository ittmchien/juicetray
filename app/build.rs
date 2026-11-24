use std::env;
use std::path::PathBuf;

// Embeds the Windows manifest (linker args, so a macOS host needs no tools) and, when the
// build host is Windows, the app icon resource; other targets get nothing.
fn main() {
    let packaging =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"))
            .join("../packaging/windows");
    for file in ["juicetray.manifest", "juicetray.ico"] {
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
    {
        let resource = version_resource(&packaging);
        embed_resource::compile(resource, embed_resource::NONE)
            .manifest_optional()
            .expect("compile the Windows icon and version resource");
    }
}

/// The icon script plus a VERSIONINFO block, so Task Manager and file Properties show
/// "Juicetray" and the crate version instead of the exe's file name.
#[cfg(windows)]
fn version_resource(packaging: &std::path::Path) -> PathBuf {
    let version = env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION");
    let numbers: Vec<&str> = version.split(['.', '-']).take(3).collect();
    let [major, minor, patch] = [0, 1, 2].map(|i| numbers.get(i).copied().unwrap_or("0"));
    let icon = packaging
        .join("juicetray.ico")
        .display()
        .to_string()
        .replace('\\', "\\\\");
    let script = format!(
        r#"1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "FileDescription", "Juicetray"
      VALUE "ProductName", "Juicetray"
      VALUE "CompanyName", "ittmchien"
      VALUE "FileVersion", "{version}"
      VALUE "ProductVersion", "{version}"
      VALUE "OriginalFilename", "juicetray.exe"
      VALUE "LegalCopyright", "GPL-3.0-or-later"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    let out = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("juicetray.rc");
    std::fs::write(&out, script).expect("write the generated resource script");
    out
}
