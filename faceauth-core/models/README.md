# dlib face models

Pre-trained models by Davis King, copied unmodified from
<https://github.com/davisking/dlib-models> (identical to the copies on
<http://dlib.net/files/>). Both are released into the public domain; see
`LICENSE-CC0`.

| File | Purpose | SHA-256 (compressed) | SHA-256 (decompressed) |
|---|---|---|---|
| `shape_predictor_5_face_landmarks.dat.bz2` | 5-point landmark predictor | `6e787bbebf5c9efdb793f6cd1f023230c4413306605f24f299f12869f95aa472` | `c4b1e9804792707d3a405c2c16a80a20269e6675021f64a41d30fffafbc41888` |
| `dlib_face_recognition_resnet_model_v1.dat.bz2` | 128-D face descriptor network | `abb1f61041e434465855ce81c2bd546e830d28bcbed8d27ffbe5bb408b11553a` | `55533b28a95800a551ba546ba62fe69625c7e95a7061c338adffead08719da30` |

`build.rs` verifies both hashes, decompresses the files into `OUT_DIR`, and
the daemon embeds them with `include_bytes!`. A mismatch fails the build.

These are the latest official dlib face models (checked 2026-11). The
DenseNet model in dlib-models (`face_recognition_densenet_model_v1`) is not a
successor: it scores lower on LFW (96.1% vs 99.38%) and needs a different
network definition.

To update: replace the `.bz2` file and update the hashes here and in
`build.rs`.
