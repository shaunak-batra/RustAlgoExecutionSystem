fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Use the vendored protoc so the build does not depend on a system install.
    std::env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path()?);

    // Generated code is written to OUT_DIR, never into the source tree.
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&["../../proto/execution.proto"], &["../../proto"])?;

    println!("cargo:rerun-if-changed=../../proto/execution.proto");

    Ok(())
}
