use std::{env, fs, path::PathBuf};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let built =
        manifest.join("../../ebpf/target/bpfel-unknown-none/release/chronicle-ebpf-capture");
    let fallback = manifest.join("objects/chronicle-ebpf-capture-bpfel.o");
    let object = if built.exists() { built } else { fallback };
    println!("cargo:rerun-if-changed={}", object.display());
    fs::copy(
        &object,
        PathBuf::from(env::var("OUT_DIR").expect("output directory"))
            .join("chronicle-ebpf-capture-bpfel.o"),
    )
    .expect("copy embedded eBPF object");
}
