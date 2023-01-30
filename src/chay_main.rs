use chay::chayd_service_client::ChaydServiceClient;
use chay::ChaydServiceGetHealthRequest;

pub mod chay {
    tonic::include_proto!("chay");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = ChaydServiceClient::connect("http://[::1]:50051").await?;
    let request = tonic::Request::new(ChaydServiceGetHealthRequest {});
    let response = client.get_health(request).await?;
    println!("RESPONSE={:?}", response);
    Ok(())
}
