fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Compile proto files
    tonic_build::configure()
        .build_server(true)
        .build_client(true)  // Enable client generation
        .out_dir("src/proto")
        .compile_protos(
            &["../../proto/execution.proto"],
            &["../../proto"],
        )?;

    // Tell cargo to rerun if proto file changes
    println!("cargo:rerun-if-changed=../../proto/execution.proto");

    Ok(())
}
