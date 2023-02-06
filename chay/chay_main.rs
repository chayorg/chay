use chay_proto::chayd_service_client::ChaydServiceClient;
use chay_proto::{ChaydServiceGetHealthRequest, ChaydServiceGetStatusRequest};

pub mod chay_proto {
    tonic::include_proto!("chay.proto.v1");
}

async fn stream_program_statuses(
    client: &mut ChaydServiceClient<tonic::transport::Channel>,
) -> Result<(), Box<dyn std::error::Error>> {
    let request = ChaydServiceGetStatusRequest {};
    let mut stream = client
        .get_status(tonic::Request::new(request))
        .await?
        .into_inner();
    while let Some(response) = stream.message().await? {
        println!("{:?}", response);
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = ChaydServiceClient::connect("http://[::1]:50051").await?;

    let get_health_request = tonic::Request::new(ChaydServiceGetHealthRequest {});
    let get_health_response = client.get_health(get_health_request).await?;
    println!("get_health_response={:?}", get_health_response);

    stream_program_statuses(&mut client).await?;

    Ok(())
}
