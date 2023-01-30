use chay_proto::chayd_service_client::ChaydServiceClient;
use chay_proto::ChaydServiceGetHealthRequest;

pub mod chay_proto {
    tonic::include_proto!("chay.proto.v1");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = ChaydServiceClient::connect("http://[::1]:50051").await?;
    let request = tonic::Request::new(ChaydServiceGetHealthRequest {});
    let response = client.get_health(request).await?;
    println!("RESPONSE={:?}", response);
    Ok(())
}
