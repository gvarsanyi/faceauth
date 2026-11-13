# vendor/

This directory holds locally patched copies of upstream crates, applied via
`[patch.crates-io]` in the workspace `Cargo.toml`. Dependencies that need no
changes are pulled from crates.io as usual; only crates with project-specific
fixes live here.

---

## nokhwa-bindings-linux

**Upstream:** [`nokhwa-bindings-linux` v0.1.3](https://crates.io/crates/nokhwa-bindings-linux)
([source](https://github.com/l1npengtul/nokhwa), Apache-2.0)

**Role:** Linux V4L2 capture backend for the `nokhwa` camera library. Handles
camera enumeration, format negotiation, and frame capture via memory-mapped
I/O. It is not a direct dependency of faceauth: it is the platform backend
pulled in transitively by `nokhwa`.

**Why patched:** The upstream crate hardcoded the FourCC code `"GRAY"` for
greyscale pixel formats. The Linux V4L2 kernel API uses `"GREY"` (the
canonical FourCC spelling). This mismatch caused `open_camera` to fail
silently on any camera that advertises only greyscale formats, which is
exactly the case for IR cameras used by Windows Hello and similar hardware.

**What changed:** The FourCC string `"GRAY"` was corrected to `"GREY"` in the
format conversion code (`fourcc_to_frameformat` / `frameformat_to_fourcc` in
`src/lib.rs`).

**How to update:** If a future `nokhwa-bindings-linux` release fixes the
upstream bug, remove this directory and the `[patch.crates-io]` entry from the
workspace `Cargo.toml`. To re-vendor a newer version:

```sh
# Download the new version into vendor/
cargo vendor vendor --no-delete
# Then re-apply the GREY fix and update [patch.crates-io] if needed.
```

---

## dlib-face-recognition

**Upstream:** [`dlib-face-recognition` v0.3.2](https://crates.io/crates/dlib-face-recognition)
([source](https://github.com/ulagbulag/dlib-face-recognition), BSD-3-Clause).
No upstream release since 2023-10.

**Why patched:** dlib 20 changed `full_object_detection` to store landmark
parts as `dpoint` (double) instead of `point` (long). The crate's
`Deref<Target = [Point]>` for `FaceLandmarks` returned a pointer into that
vector and no longer compiles; with a cast it would read garbage.

**What changed:** `src/landmark_prediction/landmarks.rs` replaces the slice
`Deref` with `len()`, `is_empty()`, and `get(index) -> Option<Point>`, which
convert each part to `dlib::point` in C++. This builds against dlib 19.x and
20.x. faceauth does not read landmark coordinates itself.

**How to update:** if upstream releases a dlib 20 compatible version, remove
this directory and its `[patch.crates-io]` entry.
