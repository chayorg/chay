use clap::Parser;
use futures_core;
use nix::sys::signal::Signal;
use nix::unistd::Pid;
use std::collections::{BTreeMap, HashMap};
use std::pin::Pin;
use tokio_stream;
use toml;
use tonic;
use wildmatch::WildMatch;

use chay_proto::chayd_service_server::{ChaydService, ChaydServiceServer};
use chay_proto::{
    ChaydServiceGetHealthRequest, ChaydServiceGetHealthResponse, ChaydServiceGetStatusRequest,
    ChaydServiceGetStatusResponse, ChaydServiceRestartRequest, ChaydServiceRestartResponse,
    ChaydServiceStartRequest, ChaydServiceStartResponse, ChaydServiceStopRequest,
    ChaydServiceStopResponse,
};

pub mod chay_proto {
    tonic::include_proto!("chay.proto.v1");
}

/// Daemon to supervise a list of processes
#[derive(clap::Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the config file
    config_path: std::path::PathBuf,
}

fn bug_panic(message: &str) {
    panic!("Internal Error! Please create a bug report: {}", message);
}

#[derive(Clone, Debug, serde::Deserialize)]
struct ProgramConfig {
    command: String,
    args: Option<Vec<String>>,
    autostart: Option<bool>,
    autorestart: Option<bool>,
    /// Seconds to wait after a program exits unexpectedly before attempted to restart the program.
    backoff_delay: Option<u32>,
    num_restart_attempts: Option<u32>,
    sigkill_delay: Option<u32>,
}

impl ProgramConfig {
    pub fn command(&self) -> String {
        self.command.clone()
    }

    pub fn args(&self) -> Option<Vec<String>> {
        self.args.clone()
    }

    pub fn autostart(&self) -> bool {
        self.autostart.unwrap_or(true)
    }

    pub fn autorestart(&self) -> bool {
        self.autorestart.unwrap_or(true)
    }

    pub fn backoff_delay(&self) -> u32 {
        self.backoff_delay.unwrap_or(1u32)
    }

    pub fn num_restart_attempts(&self) -> u32 {
        self.num_restart_attempts.unwrap_or(4u32)
    }

    pub fn sigkill_delay(&self) -> u32 {
        self.sigkill_delay.unwrap_or(10u32)
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
struct Config {
    /// List of programs from the config file, sorted by key in alphabetical order.
    programs: BTreeMap<String, ProgramConfig>,
}

#[derive(Debug)]
struct Program {
    name: String,
    config: ProgramConfig,
    num_restarts: u32,
    child_proc: Option<std::process::Child>,
    should_restart: bool,
}

impl Program {
    pub fn new(name: String, config: ProgramConfig) -> Program {
        Program {
            name,
            config,
            num_restarts: 0u32,
            child_proc: None,
            should_restart: false,
        }
    }

    pub fn name(&self) -> String {
        self.name.clone()
    }

    pub fn start(&mut self) -> std::io::Result<()> {
        self.reset_child_proc();
        let mut command = std::process::Command::new(&self.config.command());
        if let Some(args) = &self.config.args() {
            command.args(args);
        }
        match command.spawn() {
            Ok(child_proc) => {
                self.child_proc.replace(child_proc);
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    pub fn reset_child_proc(&mut self) {
        self.child_proc = None;
    }

    pub fn is_running(&mut self) -> bool {
        if let Some(child_proc) = &mut self.child_proc {
            return match child_proc.try_wait() {
                Ok(None) => true,
                Ok(Some(_)) | Err(_) => false,
            };
        }
        false
    }
}

fn read_config(config_path: &std::path::PathBuf) -> std::io::Result<Config> {
    let content = std::fs::read_to_string(config_path)?;
    Ok(toml::from_str(&content)?)
}

trait State<StateKey, AppContext, Event> {
    fn update(&mut self, _context: &mut dyn Context<StateKey>, _app_context: &mut AppContext) {}

    fn react(
        &mut self,
        _event: &Event,
        _context: &mut dyn Context<StateKey>,
        _app_context: &mut AppContext,
    ) -> MachineResult {
        Ok(None)
    }

    fn enter(&mut self, _app_context: &mut AppContext) {}
    fn exit(&mut self, _app_context: &mut AppContext) {}
}

trait Context<StateKey> {
    fn transition(&mut self, state_key: StateKey);
}

struct ContextImpl<StateKey> {
    current_state_key: StateKey,
}

impl<StateKey> ContextImpl<StateKey> {
    fn new(init_state_key: StateKey) -> Self {
        ContextImpl::<StateKey> {
            current_state_key: init_state_key,
        }
    }
}

impl<StateKey> Context<StateKey> for ContextImpl<StateKey> {
    fn transition(&mut self, state_key: StateKey) {
        self.current_state_key = state_key;
    }
}

type MachineResult = std::result::Result<Option<String>, String>;

struct Machine<StateKey, AppContext, Event> {
    app_context: AppContext,
    states: HashMap<StateKey, Box<dyn State<StateKey, AppContext, Event>>>,
    context: ContextImpl<StateKey>,
    first_update: bool,
}

impl<StateKey, AppContext, Event> Machine<StateKey, AppContext, Event>
where
    StateKey: Clone + Eq + std::hash::Hash,
{
    pub fn new(
        app_context: AppContext,
        init_state: StateKey,
        states: HashMap<StateKey, Box<dyn State<StateKey, AppContext, Event>>>,
    ) -> Self {
        return Machine::<StateKey, AppContext, Event> {
            app_context,
            states,
            context: ContextImpl::<StateKey>::new(init_state),
            first_update: true,
        };
    }

    pub fn current_state_key(&self) -> StateKey {
        return self.context.current_state_key.clone();
    }

    pub fn app_context(&self) -> &AppContext {
        &self.app_context
    }

    pub fn update(&mut self) {
        self.maybe_enter_on_first_update();
        let state_key = self.current_state_key();
        let state = self.states.get_mut(&state_key).unwrap();
        state.update(&mut self.context, &mut self.app_context);
        self.maybe_change_state(state_key);
    }

    pub fn react(&mut self, event: &Event) -> MachineResult {
        self.maybe_enter_on_first_update();
        let state_key = self.current_state_key();
        let state = self.states.get_mut(&state_key).unwrap();
        let result = state.react(event, &mut self.context, &mut self.app_context);
        self.maybe_change_state(state_key);
        result
    }

    fn maybe_enter_on_first_update(&mut self) {
        let state_key = self.current_state_key();
        {
            let state = self.states.get_mut(&state_key).unwrap();
            if self.first_update {
                self.first_update = false;
                // Ensure we call the enter method for the initial state before doing anything.
                state.enter(&mut self.app_context);
            }
        }
    }

    fn maybe_change_state(&mut self, old_state_key: StateKey) {
        let new_state_key = self.current_state_key();
        if new_state_key != old_state_key {
            {
                let old_state = self.states.get_mut(&old_state_key).unwrap();
                old_state.exit(&mut self.app_context);
            }
            let new_state = self.states.get_mut(&new_state_key).unwrap();
            new_state.enter(&mut self.app_context);
        }
    }
}

fn new_fsm(
    program_name: String,
    config: ProgramConfig,
) -> Machine<ProgramState, Program, ProgramEvent> {
    let program = Program::new(program_name, config.clone());
    let init_state = if config.autostart() {
        ProgramState::Starting
    } else {
        ProgramState::Stopped
    };
    let stopped: Box<dyn State<ProgramState, Program, ProgramEvent>> = Box::new(Stopped::default());
    let exited: Box<dyn State<ProgramState, Program, ProgramEvent>> = Box::new(Exited::default());
    let backoff: Box<dyn State<ProgramState, Program, ProgramEvent>> = Box::new(Backoff::default());
    let starting: Box<dyn State<ProgramState, Program, ProgramEvent>> =
        Box::new(Starting::default());
    let running: Box<dyn State<ProgramState, Program, ProgramEvent>> = Box::new(Running::default());
    let stopping: Box<dyn State<ProgramState, Program, ProgramEvent>> =
        Box::new(Stopping::default());
    return Machine::<ProgramState, Program, ProgramEvent>::new(
        program,
        init_state,
        HashMap::from([
            (ProgramState::Stopped, stopped),
            (ProgramState::Exited, exited),
            (ProgramState::Backoff, backoff),
            (ProgramState::Starting, starting),
            (ProgramState::Running, running),
            (ProgramState::Stopping, stopping),
        ]),
    );
}

fn transition_to_backoff_or_exited(context: &mut dyn Context<ProgramState>, program: &mut Program) {
    if program.config.autorestart() && program.num_restarts < program.config.num_restart_attempts()
    {
        context.transition(ProgramState::Backoff);
    } else {
        context.transition(ProgramState::Exited);
    }
}

enum ProgramEvent {
    Start,
    Stop,
    Restart,
}

#[derive(Clone, Eq, Hash, PartialEq)]
enum ProgramState {
    Stopped,
    Exited,
    Backoff,
    Starting,
    Running,
    Stopping,
}

#[derive(Default)]
struct Stopped {}

#[derive(Default)]
struct Exited {}

#[derive(Default)]
struct Backoff {
    enter_time: Option<std::time::Instant>,
}

#[derive(Default)]
struct Starting {}

#[derive(Default)]
struct Running {}

#[derive(Default)]
struct Stopping {
    sigterm_time: Option<std::time::Instant>,
}

impl State<ProgramState, Program, ProgramEvent> for Stopped {
    fn react(
        &mut self,
        event: &ProgramEvent,
        context: &mut dyn Context<ProgramState>,
        _program: &mut Program,
    ) -> MachineResult {
        match event {
            ProgramEvent::Start => {
                context.transition(ProgramState::Starting);
                MachineResult::Ok(None)
            }
            ProgramEvent::Stop => MachineResult::Ok(Some("Already stopped".to_string())),
            ProgramEvent::Restart => {
                context.transition(ProgramState::Starting);
                MachineResult::Ok(Some("Wasn't running (was stopped)".to_string()))
            }
        }
    }

    fn enter(&mut self, program: &mut Program) {
        program.num_restarts = 0u32;
        program.reset_child_proc();
        println!("{} stopped", program.name);
    }
}

impl State<ProgramState, Program, ProgramEvent> for Exited {
    fn react(
        &mut self,
        event: &ProgramEvent,
        context: &mut dyn Context<ProgramState>,
        _program: &mut Program,
    ) -> MachineResult {
        match event {
            ProgramEvent::Start => {
                context.transition(ProgramState::Starting);
                MachineResult::Ok(None)
            }
            ProgramEvent::Stop => MachineResult::Ok(Some("Already stopped (exited)".to_string())),
            ProgramEvent::Restart => {
                context.transition(ProgramState::Starting);
                MachineResult::Ok(Some("Wasn't running (was exited)".to_string()))
            }
        }
    }

    fn enter(&mut self, program: &mut Program) {
        program.num_restarts = 0u32;
        program.reset_child_proc();
        println!("{} exited", program.name);
    }
}

impl State<ProgramState, Program, ProgramEvent> for Backoff {
    fn update(&mut self, context: &mut dyn Context<ProgramState>, program: &mut Program) {
        let now = std::time::Instant::now();
        if (now - self.enter_time.unwrap())
            >= std::time::Duration::from_secs(program.config.backoff_delay() as u64)
        {
            context.transition(ProgramState::Starting);
        }
    }

    fn react(
        &mut self,
        event: &ProgramEvent,
        context: &mut dyn Context<ProgramState>,
        program: &mut Program,
    ) -> MachineResult {
        match event {
            ProgramEvent::Start => {
                // Reset num_restarts since the client explicitly told us to start again.
                program.num_restarts = 0u32;
                context.transition(ProgramState::Starting);
                MachineResult::Ok(Some("Already starting (was backoff)".to_string()))
            }
            ProgramEvent::Stop => {
                context.transition(ProgramState::Stopping);
                MachineResult::Ok(None)
            }
            ProgramEvent::Restart => {
                // Reset num_restarts since the client explicitly told us to start again.
                program.num_restarts = 0u32;
                context.transition(ProgramState::Starting);
                MachineResult::Ok(Some("Wasn't running (was backoff)".to_string()))
            }
        }
    }

    fn enter(&mut self, program: &mut Program) {
        println!(
            "{} backoff (delay: {})",
            program.name,
            program.config.backoff_delay()
        );
        program.num_restarts += 1u32;
        self.enter_time.replace(std::time::Instant::now());
    }
}

impl State<ProgramState, Program, ProgramEvent> for Starting {
    fn update(&mut self, context: &mut dyn Context<ProgramState>, program: &mut Program) {
        if program.is_running() {
            context.transition(ProgramState::Running);
        } else {
            transition_to_backoff_or_exited(context, program);
        }
    }

    fn react(
        &mut self,
        event: &ProgramEvent,
        context: &mut dyn Context<ProgramState>,
        program: &mut Program,
    ) -> MachineResult {
        match event {
            ProgramEvent::Start => {
                // Reset num_restarts since the client explicitly told us to start again.
                program.num_restarts = 0u32;
                context.transition(ProgramState::Starting);
                MachineResult::Ok(Some("Already starting".to_string()))
            }
            ProgramEvent::Stop => {
                context.transition(ProgramState::Stopping);
                MachineResult::Ok(None)
            }
            ProgramEvent::Restart => {
                // Reset num_restarts since the client explicitly told us to start again.
                program.num_restarts = 0u32;
                MachineResult::Ok(Some(
                    "Already starting (resetting backoff counter)".to_string(),
                ))
            }
        }
    }

    fn enter(&mut self, program: &mut Program) {
        println!("{} starting", program.name);
        if let Err(error) = program.start() {
            println!("{} spawn error: {error}", program.name);
        }
    }
}

impl State<ProgramState, Program, ProgramEvent> for Running {
    fn update(&mut self, context: &mut dyn Context<ProgramState>, program: &mut Program) {
        if let Some(child_proc) = &mut program.child_proc {
            match child_proc.try_wait() {
                Ok(None) => {
                    // Running. Nothing to do.
                }
                Ok(Some(_)) | Err(_) => {
                    transition_to_backoff_or_exited(context, program);
                }
            }
            return;
        }
        unreachable!("Child proc is None in Running state");
    }

    fn react(
        &mut self,
        event: &ProgramEvent,
        context: &mut dyn Context<ProgramState>,
        program: &mut Program,
    ) -> MachineResult {
        match event {
            ProgramEvent::Start => MachineResult::Ok(Some("Already running".to_string())),
            ProgramEvent::Stop => {
                program.should_restart = false;
                context.transition(ProgramState::Stopping);
                MachineResult::Ok(None)
            }
            ProgramEvent::Restart => {
                program.should_restart = true;
                MachineResult::Ok(None)
            }
        }
    }

    fn enter(&mut self, program: &mut Program) {
        program.num_restarts = 0u32;
        println!("{} running", program.name);
    }
}

impl Stopping {
    fn kill_program(pid: Pid, program_name: &str, context: &mut dyn Context<ProgramState>) {
        println!("Sending SIGKILL to program {}", program_name);
        match nix::sys::signal::kill(pid, Signal::SIGKILL) {
            Ok(_) => {}
            Err(e) => {
                println!(
                    "Could not send SIGKILL to program {}: {:?}",
                    program_name, e
                );
            }
        }
        // Always transition to Stopped, even if sending SIGKILL failed. Presumably that would only
        // happen if the program has already terminated somehow. I am not sure if this is actually
        // possible.
        context.transition(ProgramState::Stopped);
    }

    fn transition_to_stopped_or_restart(
        should_restart: bool,
        context: &mut dyn Context<ProgramState>,
    ) {
        if should_restart {
            context.transition(ProgramState::Starting);
        } else {
            context.transition(ProgramState::Stopped);
        }
    }
}

impl State<ProgramState, Program, ProgramEvent> for Stopping {
    fn update(&mut self, context: &mut dyn Context<ProgramState>, program: &mut Program) {
        let program_name = program.name();
        if let Some(child_proc) = &mut program.child_proc {
            match child_proc.try_wait() {
                Ok(None) => {
                    if let Some(sigterm_time) = self.sigterm_time {
                        // We already sent SIGTERM in a previous update. Send SIGKILL if it
                        // doesn't shut down within a reasonable timeout.
                        let now = std::time::Instant::now();
                        let sigkill_timeout =
                            std::time::Duration::from_secs(program.config.sigkill_delay() as u64);
                        if (now - sigterm_time) >= sigkill_timeout {
                            let pid = Pid::from_raw(child_proc.id() as i32);
                            Stopping::kill_program(pid, &program_name, context);
                            Stopping::transition_to_stopped_or_restart(
                                program.should_restart,
                                context,
                            );
                        }
                    } else {
                        // We haven't sent SIGTERM yet, so do that now.
                        self.sigterm_time = Some(std::time::Instant::now());
                        let pid = Pid::from_raw(child_proc.id() as i32);
                        match nix::sys::signal::kill(pid, Signal::SIGTERM) {
                            Ok(_) => {}
                            Err(e) => {
                                println!(
                                    "Could not send SIGTERM to program {}: {:?}",
                                    program_name, e
                                );
                                Stopping::kill_program(pid, &program_name, context);
                                Stopping::transition_to_stopped_or_restart(
                                    program.should_restart,
                                    context,
                                );
                            }
                        }
                    }
                }
                Ok(Some(_)) | Err(_) => {
                    Stopping::transition_to_stopped_or_restart(program.should_restart, context);
                }
            }
        } else {
            Stopping::transition_to_stopped_or_restart(program.should_restart, context);
        }
    }

    fn react(
        &mut self,
        event: &ProgramEvent,
        _context: &mut dyn Context<ProgramState>,
        program: &mut Program,
    ) -> MachineResult {
        match event {
            ProgramEvent::Start => MachineResult::Err("Cannot start while stopping".to_string()),
            ProgramEvent::Stop => {
                program.should_restart = false;
                MachineResult::Ok(Some("Already stopping".to_string()))
            }
            ProgramEvent::Restart => {
                program.should_restart = true;
                MachineResult::Ok(Some("Will restart after stopping".to_string()))
            }
        }
    }

    fn enter(&mut self, program: &mut Program) {
        self.sigterm_time = None;
        program.num_restarts = 0u32;
        println!("{} stopping", program.name);
    }

    fn exit(&mut self, program: &mut Program) {
        program.should_restart = false;
    }
}

#[derive(Debug, Default)]
struct ProgramStatesChannels {
    senders:
        HashMap<std::net::SocketAddr, tokio::sync::mpsc::Sender<HashMap<String, ProgramState>>>,
}

impl ProgramStatesChannels {
    async fn broadcast(&self, program_states: &HashMap<String, ProgramState>) {
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

type ProgramEventsResult = Result<HashMap<String, MachineResult>, tonic::Status>;

#[derive(Debug)]
struct ChaydServiceServerImpl {
    program_states_channels: std::sync::Arc<tokio::sync::RwLock<ProgramStatesChannels>>,
    program_events_sender: tokio::sync::mpsc::Sender<(
        ProgramEvent,
        String,
        tokio::sync::mpsc::Sender<ProgramEventsResult>,
    )>,
}

impl ChaydServiceServerImpl {
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

fn proto_from_program_state(program_state: ProgramState) -> chay_proto::ProgramState {
    match program_state {
        ProgramState::Stopped => chay_proto::ProgramState::Stopped,
        ProgramState::Exited => chay_proto::ProgramState::Exited,
        ProgramState::Backoff => chay_proto::ProgramState::Backoff,
        ProgramState::Starting => chay_proto::ProgramState::Starting,
        ProgramState::Running => chay_proto::ProgramState::Running,
        ProgramState::Stopping => chay_proto::ProgramState::Stopping,
    }
}

fn proto_program_event_result_from_machine_result(
    machine_result: &MachineResult,
) -> chay_proto::ProgramEventResult {
    match machine_result {
        Ok(Some(message)) => chay_proto::ProgramEventResult {
            result: Some(chay_proto::program_event_result::Result::Ok(
                chay_proto::program_event_result::Ok {
                    message: message.clone(),
                },
            )),
        },
        Ok(None) => chay_proto::ProgramEventResult {
            result: Some(chay_proto::program_event_result::Result::Ok(
                chay_proto::program_event_result::Ok::default(),
            )),
        },
        Err(message) => chay_proto::ProgramEventResult {
            result: Some(chay_proto::program_event_result::Result::Err(
                chay_proto::program_event_result::Err {
                    message: message.clone(),
                },
            )),
        },
    }
}

fn proto_start_response_from_program_events_results(
    program_events_results: &HashMap<String, MachineResult>,
) -> ChaydServiceStartResponse {
    let mut response = ChaydServiceStartResponse::default();
    program_events_results
        .iter()
        .for_each(|(program_name, machine_result)| {
            response.program_event_results.insert(
                program_name.clone(),
                proto_program_event_result_from_machine_result(&machine_result),
            );
        });
    response
}

fn proto_stop_response_from_program_events_results(
    program_events_results: &HashMap<String, MachineResult>,
) -> ChaydServiceStopResponse {
    let mut response = ChaydServiceStopResponse::default();
    program_events_results
        .iter()
        .for_each(|(program_name, machine_result)| {
            response.program_event_results.insert(
                program_name.clone(),
                proto_program_event_result_from_machine_result(&machine_result),
            );
        });
    response
}

fn proto_restart_response_from_program_events_results(
    program_events_results: &HashMap<String, MachineResult>,
) -> ChaydServiceRestartResponse {
    let mut response = ChaydServiceRestartResponse::default();
    program_events_results
        .iter()
        .for_each(|(program_name, machine_result)| {
            response.program_event_results.insert(
                program_name.clone(),
                proto_program_event_result_from_machine_result(&machine_result),
            );
        });
    response
}

#[tonic::async_trait]
impl ChaydService for ChaydServiceServerImpl {
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

fn update_program_fsms(program_fsms: &mut Vec<Machine<ProgramState, Program, ProgramEvent>>) {
    for program_fsm in program_fsms {
        program_fsm.update();
    }
}

async fn broadcast_program_states(
    program_fsms: &Vec<Machine<ProgramState, Program, ProgramEvent>>,
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let config = read_config(&args.config_path).unwrap_or_else(|error| {
        println!("Error parsing toml file: {}", error);
        std::process::exit(1);
    });

    let mut program_fsms: Vec<Machine<ProgramState, Program, ProgramEvent>> = config
        .programs
        .iter()
        .map(|(program_name, program_config)| new_fsm(program_name.clone(), program_config.clone()))
        .collect();

    let program_states_channels =
        std::sync::Arc::new(tokio::sync::RwLock::new(ProgramStatesChannels::default()));
    let (program_events_tx, mut program_events_rx) = tokio::sync::mpsc::channel(20);

    let chayd_addr = "[::1]:50051".parse()?;
    let chayd_server =
        ChaydServiceServerImpl::new(program_states_channels.clone(), program_events_tx);

    tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(ChaydServiceServer::new(chayd_server))
            .serve(chayd_addr),
    );

    let mut fsm_update_interval = tokio::time::interval(tokio::time::Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = fsm_update_interval.tick() => {
                update_program_fsms(&mut program_fsms);
                broadcast_program_states(&program_fsms, &program_states_channels).await;
            },
            Some((program_event, program_expr, program_events_tx)) = program_events_rx.recv() => {
                let mut result = HashMap::<String, MachineResult>::new();
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
                        Err(_) => println!("Warning: Could not send program events results"),
                    }
                } else {
                    match program_events_tx.send(Ok(result)).await {
                        Ok(_) => {},
                        // The connection was probably closed by the client.
                        Err(_) => println!("Warning: Could not send program events results"),
                    }
                }
                broadcast_program_states(&program_fsms, &program_states_channels).await;
            }
        }
    }
}
