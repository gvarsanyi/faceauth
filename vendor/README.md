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
I/O. It is not a direct dependency of faceauth — it is the platform backend
pulled in transitively by `nokhwa`.

**Why patched:** The upstream crate hardcoded the FourCC code `"GRAY"` for
greyscale pixel formats. The Linux V4L2 kernel API uses `"GREY"` (the
canonical FourCC spelling). This mismatch caused `open_camera` to fail
silently on any camera that advertises only greyscale formats — which is
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
