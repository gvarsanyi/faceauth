//! Model files are written by the daemon only, but parsing must still never
//! panic, and anything accepted must survive a save/load round trip.
#![no_main]

use faceauth_core::store::{model_from_json, model_to_json};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(json) = std::str::from_utf8(data) else { return };
    if let Ok(model) = model_from_json(json) {
        let again = model_from_json(&model_to_json(&model)).expect("serialized model must parse");
        // Compare bit patterns: NaN encodings are valid f32 bits but not equal.
        let bits = |m: &faceauth_core::store::FaceModel| {
            m.batches.iter().flatten().flat_map(|e| e.iter().map(|v| v.to_bits())).collect::<Vec<_>>()
        };
        assert_eq!(model.camera, again.camera);
        assert_eq!(bits(&model), bits(&again));
    }
});
