fn main() {
    let proto_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../proto");
    println!("cargo:rerun-if-changed={}", proto_dir.display());
    let v1 = proto_dir.join("mediaops/v1/mediaops.proto");
    let home = proto_dir.join("mediaops/home/v1/home.proto");
    tonic_prost_build::configure()
        .compile_protos(&[v1], std::slice::from_ref(&proto_dir))
        .expect("compile seedbox proto");
    let home_out =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("home");
    std::fs::create_dir_all(&home_out).expect("home generated directory");
    tonic_prost_build::configure()
        .out_dir(home_out)
        .extern_path(".mediaops.v1", "crate")
        .compile_protos(&[home], &[proto_dir])
        .expect("compile mediaops protos");
}
