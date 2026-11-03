//! Compiles the bundled icons (`data/icons/*.svg`) into a GResource, which
//! `ui::icons::register` loads at startup. On Windows it also gives the program its
//! icon and name.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

use gvdb::gresource::{BundleBuilder, FileData, PreprocessOptions};

const ICONS_DIR: &str = "data/icons";
/// Where the icons go inside the resource. GTK looks them up in the hicolor theme's
/// folder layout under the path `ui::icons` registers (`/penguin-player/icons`).
const RESOURCE_FOLDER: &str = "/penguin-player/icons/scalable/actions";
/// The program's icon on Windows: the penguin from 16 to 256 pixels.
const WINDOWS_ICON: &str = "data/windows/penguin-player.ico";

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo::rerun-if-changed={ICONS_DIR}");
    let mut files = Vec::new();
    for svg in svg_files(Path::new(ICONS_DIR))? {
        let name = svg
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("{} has no UTF-8 file name", svg.display()))?;
        let key = format!("{RESOURCE_FOLDER}/{name}");
        let options = PreprocessOptions::xml_stripblanks();
        files.push(FileData::from_file(key, &svg, false, &options)?);
    }
    // Sorted so that builds are reproducible.
    files.sort();
    let resource = BundleBuilder::from_file_data(files).build()?;
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    fs::write(out_dir.join("icons.gresource"), resource)?;

    if env::var("CARGO_CFG_TARGET_OS")? == "windows" {
        windows_resources(&out_dir)?;
    }
    Ok(())
}

/// Builds the program's icon and its name, which Explorer, the taskbar and Task Manager
/// show, into it as Windows resources. They are compiled by `windres`, which comes with
/// MSYS2's compilers (or the program named by `WINDRES`).
fn windows_resources(out_dir: &Path) -> Result<(), Box<dyn Error>> {
    println!("cargo::rerun-if-changed={WINDOWS_ICON}");
    println!("cargo::rerun-if-env-changed=WINDRES");
    let icon = Path::new(&env::var("CARGO_MANIFEST_DIR")?).join(WINDOWS_ICON);
    // windres reads backslashes in strings as escapes.
    let icon = icon
        .to_str()
        .ok_or("the icon's path isn't UTF-8")?
        .replace('\\', "/");
    let version = env::var("CARGO_PKG_VERSION")?;
    let mut numbers = Vec::new();
    for part in ["MAJOR", "MINOR", "PATCH"] {
        numbers.push(env::var(format!("CARGO_PKG_VERSION_{part}"))?);
    }
    let numbers = numbers.join(",");
    let script = format!(
        r#"1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {numbers},0
PRODUCTVERSION {numbers},0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "FileDescription", "Penguin Player"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "penguin-player"
      VALUE "OriginalFilename", "penguin-player.exe"
      VALUE "ProductName", "Penguin Player"
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
    let script_path = out_dir.join("penguin-player.rc");
    let object = out_dir.join("penguin-player-resources.o");
    fs::write(&script_path, script)?;
    let windres = env::var("WINDRES").unwrap_or_else(|_| "windres".to_owned());
    let status = Command::new(&windres)
        .arg("--codepage=65001")
        .arg("--input")
        .arg(&script_path)
        .args(["--output-format", "coff", "--output"])
        .arg(&object)
        .status()
        .map_err(|err| {
            format!("could not run {windres}, which Windows builds need (it comes with MSYS2's compilers): {err}")
        })?;
    if !status.success() {
        return Err(format!("{windres} failed ({status})").into());
    }
    println!("cargo::rustc-link-arg-bins={}", object.display());
    Ok(())
}

fn svg_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|extension| extension == "svg") {
            files.push(path);
        }
    }
    Ok(files)
}
