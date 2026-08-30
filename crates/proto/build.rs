fn main() {
    let proto_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../proto");
    println!("cargo:rerun-if-changed={}", proto_dir.display());
    let proto_file = proto_dir.join("mediaops.proto");
    tonic_prost_build::configure()
        .compile_protos(&[proto_file], &[proto_dir])
        .expect("compile mediaops.v1 protos");
}
