fn main() -> Result<(), Box<dyn std::error::Error>> {
    let include_paths = ["."];
    tonic_build::configure().compile(
        &[
            "chay/proto/v1/chayd_service.proto",
            "chay/proto/v1/program_event_result.proto",
            "chay/proto/v1/program_status.proto",
        ],
        &include_paths,
    )?;
    Ok(())
}
