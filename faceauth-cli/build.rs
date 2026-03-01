fn main() {
    let dir = std::env::var("FACEAUTH_LIBEXEC_DIR").unwrap_or_else(|_| "/usr/libexec".to_string());
    println!("cargo:rustc-env=FACEAUTH_LIBEXEC_DIR={}", dir);
    println!("cargo:rerun-if-env-changed=FACEAUTH_LIBEXEC_DIR");
}
