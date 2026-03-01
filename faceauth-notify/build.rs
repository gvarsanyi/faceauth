use std::path::PathBuf;
use std::process::Command;

fn main() {
    let icons_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("../assets/icons");

    let svg = icons_dir.join("faceauth.svg");
    let script = icons_dir.join("generate.sh");

    // Re-run only when the master SVG or the generation script changes.
    println!("cargo:rerun-if-changed={}", svg.display());
    println!("cargo:rerun-if-changed={}", script.display());

    let status = Command::new("bash")
        .arg(&script)
        .current_dir(&icons_dir)
        .status()
        .unwrap_or_else(|e| panic!("Failed to run generate.sh: {e}"));

    if !status.success() {
        panic!("generate.sh failed — ensure ImageMagick is installed (magick command)");
    }
}
