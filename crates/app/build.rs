//! Compiles the bundled icons (`data/icons/*.svg`) into a GResource, which
//! `ui::icons::register` loads at startup.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::{env, fs};

use gvdb::gresource::{BundleBuilder, FileData, PreprocessOptions};

const ICONS_DIR: &str = "data/icons";
/// Where the icons go inside the resource. GTK looks them up in the hicolor theme's
/// folder layout under the path `ui::icons` registers (`/penguin-player/icons`).
const RESOURCE_FOLDER: &str = "/penguin-player/icons/scalable/actions";

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
