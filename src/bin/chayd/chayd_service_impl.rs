use crate::bug_panic;
use crate::chay_proto;
use crate::program_fsm::{ProgramEvent, ProgramFsm, ProgramState};
use crate::proto_converters::{
    proto_from_program_state, proto_restart_response_from_program_events_results,
    proto_start_response_from_program_events_results,
    proto_stop_response_from_program_events_results,
};
use chay_proto::chayd_service_server::ChaydService;
use chay_proto::{
    ChaydServiceGetHealthRequest, ChaydServiceGetHealthResponse, ChaydServiceGetStatusRequest,
    ChaydServiceGetStatusResponse, ChaydServiceRestartRequest, ChaydServiceRestartResponse,
    ChaydServiceStartRequest, ChaydServiceStartResponse, ChaydServiceStopRequest,
    ChaydServiceStopResponse,
};
use futures_core;
use std::collections::HashMap;
use std::pin::Pin;
use tokio_stream;
use tonic;

#[derive(Default)]
pub struct ProgramStatesChannels {
    pub senders:
        HashMap<std::net::SocketAddr, tokio::sync::mpsc::Sender<HashMap<String, ProgramState>>>,
}

impl ProgramStatesChannels {
    pub async fn broadcast(&self, program_states: &HashMap<String, ProgramState>) {
        for (socket, tx) in &self.senders {
            match tx.send(program_states.clone()).await {
                Ok(_) => {}
                Err(_) => {
                    println!("[Broadcast] SendError: to {}", socket)
                }
            }
        }
    }
}

pub async fn broadcast_program_states(
    program_fsms: &Vec<ProgramFsm>,
    program_states_channels: &std::sync::Arc<tokio::sync::RwLock<ProgramStatesChannels>>,
) {
    let program_states: HashMap<String, ProgramState> = program_fsms
        .iter()
        .map(|machine| (machine.app_context().name(), machine.current_state_key()))
        .collect();
    program_states_channels
        .read()
        .await
        .broadcast(&program_states)
        .await;
}

pub type ProgramEventsResult = Result<HashMap<String, chay::fsm::MachineResult>, tonic::Status>;

pub struct ChaydServiceImpl {
    program_states_channels: std::sync::Arc<tokio::sync::RwLock<ProgramStatesChannels>>,
    program_events_sender: tokio::sync::mpsc::Sender<(
        ProgramEvent,
        String,
        tokio::sync::mpsc::Sender<ProgramEventsResult>,
    )>,
}

impl ChaydServiceImpl {
    pub fn new(
        program_states_channels: std::sync::Arc<tokio::sync::RwLock<ProgramStatesChannels>>,
        program_events_sender: tokio::sync::mpsc::Sender<(
            ProgramEvent,
            String,
            tokio::sync::mpsc::Sender<ProgramEventsResult>,
        )>,
    ) -> Self {
        Self {
            program_states_channels,
            program_events_sender,
        }
    }
}

#[tonic::async_trait]
impl ChaydService for ChaydServiceImpl {
    type GetStatusStream = Pin<
        Box<
            dyn futures_core::Stream<Item = Result<ChaydServiceGetStatusResponse, tonic::Status>>
                + Send,
        >,
    >;

    async fn get_health(
        &self,
        request: tonic::Request<ChaydServiceGetHealthRequest>,
    ) -> Result<tonic::Response<ChaydServiceGetHealthResponse>, tonic::Status> {
        println!("Received GetHealth request: {:?}", request.get_ref());
        let response = ChaydServiceGetHealthResponse {};
        Ok(tonic::Response::new(response))
    }

    async fn get_status(
        &self,
        request: tonic::Request<ChaydServiceGetStatusRequest>,
    ) -> tonic::Result<tonic::Response<Self::GetStatusStream>, tonic::Status> {
        let remote_addr = request.remote_addr().unwrap();
        println!("GetStatus client connected from {:?}", &remote_addr);
        let (stream_tx, stream_rx) = tokio::sync::mpsc::channel(1);
        let (program_states_tx, mut program_states_rx) = tokio::sync::mpsc::channel(1);
        {
            self.program_states_channels
                .write()
                .await
                .senders
                .insert(remote_addr.clone(), program_states_tx);
        }
        let program_states_channels_clone = self.program_states_channels.clone();
        tokio::spawn(async move {
            while let Some(program_states) = program_states_rx.recv().await {
                let program_statuses_proto = program_states
                    .iter()
                    .map(|(program_name, program_state)| {
                        let mut program_status = chay_proto::ProgramStatus::default();
                        program_status.name = program_name.clone();
                        program_status.set_state(proto_from_program_state(program_state.clone()));
                        program_status
                    })
                    .collect();
                let response = ChaydServiceGetStatusResponse {
                    program_statuses: program_statuses_proto,
                };
                match stream_tx.send(tonic::Result::Ok(response)).await {
                    // response was successfully queued to be send to client.
                    Ok(_) => {}
                    // output_stream was build from rx and both are dropped
                    Err(_) => {
                        break;
                    }
                }
            }
            {
                program_states_channels_clone
                    .write()
                    .await
                    .senders
                    .remove(&remote_addr);
            }
            println!("GetStatus client disconnected from {:?}", &remote_addr);
        });

        let response_stream = tokio_stream::wrappers::ReceiverStream::new(stream_rx);
        Ok(tonic::Response::new(
            Box::pin(response_stream) as Self::GetStatusStream
        ))
    }

    async fn start(
        &self,
        request: tonic::Request<ChaydServiceStartRequest>,
    ) -> tonic::Result<tonic::Response<ChaydServiceStartResponse>, tonic::Status> {
        println!("Received Start request {:?}", request.get_ref());
        let (program_events_results_tx, mut program_events_results_rx) =
            tokio::sync::mpsc::channel(1);
        match self
            .program_events_sender
            .send((
                ProgramEvent::Start,
                request.get_ref().program_expr.clone(),
                program_events_results_tx,
            ))
            .await
        {
            Ok(_) => {}
            Err(_) => {
                bug_panic("Could not send to start channel");
            }
        }
        match program_events_results_rx.recv().await {
            Some(result) => match result {
                Ok(program_events_results) => Ok(tonic::Response::new(
                    proto_start_response_from_program_events_results(&program_events_results),
                )),
                Err(err) => Err(err),
            },
            None => {
                bug_panic("Received None from start channel rx");
                // Unreachable
                Err(tonic::Status::unknown(
                    "Received None from start channel rx",
                ))
            }
        }
    }

    async fn stop(
        &self,
        request: tonic::Request<ChaydServiceStopRequest>,
    ) -> tonic::Result<tonic::Response<ChaydServiceStopResponse>, tonic::Status> {
        println!("Received Stop request: {:?}", request.get_ref());
        let (program_events_results_tx, mut program_events_results_rx) =
            tokio::sync::mpsc::channel(1);
        match self
            .program_events_sender
            .send((
                ProgramEvent::Stop,
                request.get_ref().program_expr.clone(),
                program_events_results_tx,
            ))
            .await
        {
            Ok(_) => {}
            Err(_) => {
                bug_panic("Could not send to stop channel");
            }
        }
        match program_events_results_rx.recv().await {
            Some(result) => match result {
                Ok(program_events_results) => Ok(tonic::Response::new(
                    proto_stop_response_from_program_events_results(&program_events_results),
                )),
                Err(err) => Err(err),
            },
            None => {
                bug_panic("Received None from stop channel rx");
                // Unreachable
                Err(tonic::Status::unknown("Received None from stop channel rx"))
            }
        }
    }

    async fn restart(
        &self,
        request: tonic::Request<ChaydServiceRestartRequest>,
    ) -> tonic::Result<tonic::Response<ChaydServiceRestartResponse>, tonic::Status> {
        println!("Received Restart request: {:?}", request.get_ref());
        let (program_events_results_tx, mut program_events_results_rx) =
            tokio::sync::mpsc::channel(1);
        match self
            .program_events_sender
            .send((
                ProgramEvent::Restart,
                request.get_ref().program_expr.clone(),
                program_events_results_tx,
            ))
            .await
        {
            Ok(_) => {}
            Err(_) => {
                bug_panic("Could not send to restart channel");
            }
        }
        match program_events_results_rx.recv().await {
            Some(result) => match result {
                Ok(program_events_results) => Ok(tonic::Response::new(
                    proto_restart_response_from_program_events_results(&program_events_results),
                )),
                Err(err) => Err(err),
            },
            None => {
                bug_panic("Received None from restart channel rx");
                // Unreachable
                Err(tonic::Status::unknown(
                    "Received None from restart channel rx",
                ))
            }
        }
    }
}
