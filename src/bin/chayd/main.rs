use crate::chayd_service_impl::{
    broadcast_program_states, ChaydServiceImpl, ProgramStatesChannels,
};
use crate::program_fsm::{new_program_fsm, ProgramFsm};
use chay_proto::chayd_service_server::ChaydServiceServer;
use clap::Parser;
use std::collections::HashMap;
use wildmatch::WildMatch;

mod chay_proto {
    tonic::include_proto!("chay.proto.v1");
}
mod chayd_service_impl;
mod config;
mod program;
mod program_context;
mod program_fsm;
mod proto_converters;

/// Daemon to supervise a list of processes
#[derive(clap::Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the config file
    config_path: std::path::PathBuf,
}

pub fn bug_panic(message: &str) {
    panic!("Internal Error! Please create a bug report: {}", message);
}

fn update_program_fsms(program_fsms: &mut Vec<ProgramFsm>) {
    for program_fsm in program_fsms {
        program_fsm.update();
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let log_config = simple_log::LogConfigBuilder::builder()
        .level("info")
        .output_console()
        .build();
    simple_log::new(log_config)?;

    let args = Args::parse();
    let config = crate::config::read_from_file(&args.config_path).unwrap_or_else(|error| {
        log::error!("Error parsing toml file: {}", error);
        std::process::exit(1);
    });
    let rendered_config = crate::config::render(&config).unwrap_or_else(|error| {
        log::error!("Invalid config: {}", error.source().unwrap());
        std::process::exit(1);
    });

    let mut program_fsms: Vec<ProgramFsm> = rendered_config
        .iter()
        .map(|(program_name, program_config)| {
            new_program_fsm(program_name.clone(), &program_config)
        })
        .collect();

    let program_states_channels =
        std::sync::Arc::new(tokio::sync::RwLock::new(ProgramStatesChannels::default()));
    let (program_events_tx, mut program_events_rx) = tokio::sync::mpsc::channel(20);

    let chayd_addr = "[::1]:50051".parse()?;
    let chayd_service = ChaydServiceImpl::new(program_states_channels.clone(), program_events_tx);

    tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(ChaydServiceServer::new(chayd_service))
            .serve(chayd_addr),
    );

    let mut fsm_update_interval = tokio::time::interval(tokio::time::Duration::from_millis(500));
    loop {
        tokio::select! {
            _ = fsm_update_interval.tick() => {
                update_program_fsms(&mut program_fsms);
                broadcast_program_states(&program_fsms, &program_states_channels).await;
            },
            Some((program_event, program_expr, program_events_tx)) = program_events_rx.recv() => {
                let mut result = HashMap::<String, chay::fsm::MachineResult>::new();
                let match_all = program_expr == "all";
                for fsm in &mut program_fsms {
                    let program_name = fsm.app_context().name();
                    if match_all || WildMatch::new(&program_expr).matches(&program_name) {
                        result.insert(program_name, fsm.react(&program_event));
                    }
                }
                if result.is_empty() {
                    let status = tonic::Status::not_found(format!(
                        "No programs found matching expression: {}",
                        program_expr
                    ));
                    match program_events_tx.send(Err(status)).await {
                        Ok(_) => {},
                        // The connection was probably closed by the client.
                        Err(_) => log::warn!("Could not send program events results"),
                    }
                } else {
                    match program_events_tx.send(Ok(result)).await {
                        Ok(_) => {},
                        // The connection was probably closed by the client.
                        Err(_) => log::warn!("Could not send program events results"),
                    }
                }
                broadcast_program_states(&program_fsms, &program_states_channels).await;
            }
        }
    }
}
