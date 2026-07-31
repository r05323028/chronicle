use std::{env, fs, path::PathBuf};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let workspace_root = manifest.join("../..");
    let ebpf_target = env::var_os("CHRONICLE_EBPF_TARGET_DIR")
        .map_or_else(|| workspace_root.join("ebpf/target"), PathBuf::from);
    let ebpf_target = if ebpf_target.is_absolute() {
        ebpf_target
    } else {
        workspace_root.join(ebpf_target)
    };
    let built = ebpf_target.join("bpfel-unknown-none/release/chronicle-ebpf-capture");
    let fallback = manifest.join("objects/chronicle-ebpf-capture-bpfel.o");
    println!("cargo:rerun-if-env-changed=CHRONICLE_EBPF_TARGET_DIR");
    let object = if built.exists() { built } else { fallback };
    println!("cargo:rerun-if-changed={}", object.display());
    fs::copy(
        &object,
        PathBuf::from(env::var("OUT_DIR").expect("output directory"))
            .join("chronicle-ebpf-capture-bpfel.o"),
    )
    .expect("copy embedded eBPF object");
}
