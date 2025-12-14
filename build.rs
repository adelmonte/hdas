use libbpf_cargo::SkeletonBuilder;
use std::env;
use std::path::PathBuf;

fn main() {
    let mut out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    out.push("monitor.skel.rs");

    SkeletonBuilder::new()
        .source("bpf/monitor.bpf.c")
        .build_and_generate(&out)
        .unwrap();

    println!("cargo:rerun-if-changed=bpf/monitor.bpf.c");
}
