fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::compile_protos("chay/proto/v1/chayd_service.proto")?;
    Ok(())
}
