//! Verify the committed dlib models against pinned SHA-256 hashes and
//! decompress them into OUT_DIR for `include_bytes!`. No network access.

use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

/// (file name without .bz2, SHA-256 of the .bz2, SHA-256 of the decompressed file)
const MODELS: &[(&str, &str, &str)] = &[
    (
        "shape_predictor_5_face_landmarks.dat",
        "6e787bbebf5c9efdb793f6cd1f023230c4413306605f24f299f12869f95aa472",
        "c4b1e9804792707d3a405c2c16a80a20269e6675021f64a41d30fffafbc41888",
    ),
    (
        "dlib_face_recognition_resnet_model_v1.dat",
        "abb1f61041e434465855ce81c2bd546e830d28bcbed8d27ffbe5bb408b11553a",
        "55533b28a95800a551ba546ba62fe69625c7e95a7061c338adffead08719da30",
    ),
];

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let out_dir = std::env::var("OUT_DIR").unwrap();

    for (name, bz2_hash, dat_hash) in MODELS {
        let src = Path::new(&manifest_dir).join("models").join(format!("{name}.bz2"));
        println!("cargo:rerun-if-changed={}", src.display());

        let compressed = std::fs::read(&src).unwrap_or_else(|e| panic!("cannot read {}: {e}", src.display()));
        let actual = sha256_hex(&compressed);
        assert_eq!(&actual, bz2_hash, "checksum mismatch for {}", src.display());

        let mut data = Vec::new();
        bzip2::read::BzDecoder::new(compressed.as_slice())
            .read_to_end(&mut data)
            .unwrap_or_else(|e| panic!("cannot decompress {}: {e}", src.display()));
        let actual = sha256_hex(&data);
        assert_eq!(&actual, dat_hash, "checksum mismatch for decompressed {name}");

        let dest = Path::new(&out_dir).join(name);
        std::fs::write(&dest, &data).unwrap_or_else(|e| panic!("cannot write {}: {e}", dest.display()));
    }
}
