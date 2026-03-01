/// Build script: download and decompress the dlib pre-trained model files if
/// they are not already present under `models/`.
///
/// Downloads are performed via `curl`, which handles HTTPS and redirects.
/// The `bzip2` crate decompresses in-process.  The `.dat` files are listed in
/// `.gitignore` and must never be committed to the repository.
use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::Command;

const MODELS: &[(&str, &str)] = &[
    (
        "shape_predictor_5_face_landmarks.dat",
        "https://dlib.net/files/shape_predictor_5_face_landmarks.dat.bz2",
    ),
    (
        "dlib_face_recognition_resnet_model_v1.dat",
        "https://dlib.net/files/dlib_face_recognition_resnet_model_v1.dat.bz2",
    ),
];

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let models_dir = Path::new(&manifest_dir).join("models");

    fs::create_dir_all(&models_dir).expect("failed to create models/ directory");

    for (filename, url) in MODELS {
        let dest = models_dir.join(filename);

        // Tell Cargo to rerun this script if the file appears or disappears.
        println!("cargo:rerun-if-changed={}", dest.display());

        if dest.exists() {
            continue;
        }

        eprintln!("build: downloading {} ...", filename);
        let compressed = curl_fetch(url)
            .unwrap_or_else(|e| panic!("failed to download {url}: {e}"));

        eprintln!("build: decompressing {} ({} bytes compressed) ...", filename, compressed.len());
        let data = decompress_bz2(&compressed)
            .unwrap_or_else(|e| panic!("failed to decompress {filename}: {e}"));

        fs::write(&dest, &data)
            .unwrap_or_else(|e| panic!("failed to write {}: {e}", dest.display()));

        eprintln!("build: wrote {} ({} bytes)", dest.display(), data.len());
    }
}

/// Download a URL and return the raw bytes, using the system `curl`.
/// Follows redirects (-L) and fails on HTTP errors (-f).
fn curl_fetch(url: &str) -> Result<Vec<u8>, String> {
    let output = Command::new("curl")
        .args(["--silent", "--show-error", "--location", "--fail", "--output", "-", url])
        .output()
        .map_err(|e| format!("failed to run curl: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("curl exited with {}: {}", output.status, stderr.trim()));
    }

    Ok(output.stdout)
}

/// Decompress bzip2 data using the `bzip2` crate.
fn decompress_bz2(data: &[u8]) -> Result<Vec<u8>, String> {
    use bzip2::read::BzDecoder;
    let mut decoder = BzDecoder::new(data);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out).map_err(|e| format!("bzip2 decode: {e}"))?;
    Ok(out)
}
