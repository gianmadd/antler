// On Windows, antler.exe carries the app icon and the version info Explorer shows (Properties, Task Manager).
// The icon is resource ID 1: GPUI's windows load that one (LoadImageW(MAKEINTRESOURCE(1))).
// gpui-pre already links the manifest (resource ID 1, RT_MANIFEST): this .rc must not add a second one.
use std::{env, fs, path::Path};

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=assets/icon.ico"); // assets/icon.png at 16 to 256 px
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    // FILEVERSION takes four numbers: 0.3.0 -> 0,3,0,0 (a pre-release suffix is dropped)
    let mut numbers: Vec<String> =
        version.split(['.', '-', '+']).take(3).map(|n| n.parse::<u16>().unwrap_or(0).to_string()).collect();
    numbers.resize(4, "0".into());
    let numbers = numbers.join(",");
    let icon = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join("assets/icon.ico");
    let icon = icon.to_str().unwrap().replace('\\', "/");
    let rc = format!(
        r#"1 ICON "{icon}"

1 VERSIONINFO
FILEVERSION {numbers}
PRODUCTVERSION {numbers}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904B0"
        BEGIN
            VALUE "FileDescription", "Antler"
            VALUE "FileVersion", "{version}"
            VALUE "InternalName", "antler"
            VALUE "OriginalFilename", "antler.exe"
            VALUE "ProductName", "Antler"
            VALUE "ProductVersion", "{version}"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1200
    END
END
"#
    );
    let file = Path::new(&env::var("OUT_DIR").unwrap()).join("antler.rc");
    fs::write(&file, rc).unwrap();
    embed_resource::compile(&file, embed_resource::NONE).manifest_optional().unwrap();
}
