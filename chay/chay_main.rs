use chay_proto::chayd_service_client::ChaydServiceClient;
use chay_proto::{
    ChaydServiceGetHealthRequest, ChaydServiceGetStatusRequest, ChaydServiceRestartRequest,
    ChaydServiceStartRequest, ChaydServiceStopRequest,
};
use clap::Parser;

pub mod chay_proto {
    tonic::include_proto!("chay.proto.v1");
}

#[derive(clap::Parser)]
struct Args {
    #[command(subcommand)]
    action: Action,
}

#[derive(clap::Subcommand)]
enum Action {
    Health,
    Status,
    Start { program_expr: String },
    Stop { program_expr: String },
    Restart { program_expr: String },
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

async fn handle_health_action() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = ChaydServiceClient::connect("http://[::1]:50051").await?;
    let request = tonic::Request::new(ChaydServiceGetHealthRequest {});
    let response = client.get_health(request).await?;
    println!("{:?}", response.get_ref());
    Ok(())
}

async fn handle_status_action() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = ChaydServiceClient::connect("http://[::1]:50051").await?;
    stream_program_statuses(&mut client).await?;
    Ok(())
}

async fn handle_start_action(program_expr: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut client = ChaydServiceClient::connect("http://[::1]:50051").await?;
    let request = tonic::Request::new(ChaydServiceStartRequest {
        program_expr: program_expr.to_string(),
    });
    let response = client.start(request).await?;
    println!("{:?}", response.get_ref());
    Ok(())
}

async fn handle_stop_action(program_expr: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut client = ChaydServiceClient::connect("http://[::1]:50051").await?;
    let request = tonic::Request::new(ChaydServiceStopRequest {
        program_expr: program_expr.to_string(),
    });
    let response = client.stop(request).await?;
    println!("{:?}", response.get_ref());
    Ok(())
}

async fn handle_restart_action(program_expr: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut client = ChaydServiceClient::connect("http://[::1]:50051").await?;
    let request = tonic::Request::new(ChaydServiceRestartRequest {
        program_expr: program_expr.to_string(),
    });
    let response = client.restart(request).await?;
    println!("{:?}", response.get_ref());
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    match &args.action {
        Action::Health => handle_health_action().await,
        Action::Status => handle_status_action().await,
        Action::Start { program_expr } => handle_start_action(&program_expr).await,
        Action::Stop { program_expr } => handle_stop_action(&program_expr).await,
        Action::Restart { program_expr } => handle_restart_action(&program_expr).await,
    }
}
