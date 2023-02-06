use async_stream;
use clap::Parser;
use futures_core;
use std::collections::{BTreeMap, HashMap};
use std::pin::Pin;
use toml;
use tonic::{transport::Server, Request, Response, Status};

use chay_proto::chayd_service_server::{ChaydService, ChaydServiceServer};
use chay_proto::{
    ChaydServiceGetHealthRequest, ChaydServiceGetHealthResponse, ChaydServiceGetStatusRequest,
    ChaydServiceGetStatusResponse,
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

#[derive(Clone, Debug, serde::Deserialize)]
struct ProgramConfig {
    command: String,
    args: Option<Vec<String>>,
    autostart: Option<bool>,
    autorestart: Option<bool>,
    /// Seconds to wait after a program exits unexpectedly before attempted to restart the program.
    backoff_delay: Option<u32>,
    num_restart_attempts: Option<u32>,
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
}

impl Program {
    pub fn new(name: String, config: ProgramConfig) -> Program {
        Program {
            name,
            config,
            num_restarts: 0u32,
            child_proc: None,
        }
    }

    pub fn start(&mut self) -> std::io::Result<()> {
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

trait State<StateKey, AppContext> {
    fn update(&mut self, context: &mut dyn Context<StateKey>, app_context: &mut AppContext);
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

struct Machine<StateKey, AppContext> {
    app_context: AppContext,
    states: HashMap<StateKey, Box<dyn State<StateKey, AppContext>>>,
    context: ContextImpl<StateKey>,
    first_update: bool,
}

impl<StateKey, AppContext> Machine<StateKey, AppContext>
where
    StateKey: Clone + Eq + std::hash::Hash,
{
    fn new(
        app_context: AppContext,
        init_state: StateKey,
        states: HashMap<StateKey, Box<dyn State<StateKey, AppContext>>>,
    ) -> Self {
        return Machine::<StateKey, AppContext> {
            app_context,
            states,
            context: ContextImpl::<StateKey>::new(init_state),
            first_update: true,
        };
    }

    fn current_state_key(&self) -> StateKey {
        return self.context.current_state_key.clone();
    }

    fn update(&mut self) {
        let state_key = self.current_state_key();
        {
            let state = self.states.get_mut(&state_key).unwrap();
            if self.first_update {
                self.first_update = false;
                // Ensure we call the enter method for the initial state before doing anything.
                state.enter(&mut self.app_context);
            }
            state.update(&mut self.context, &mut self.app_context);
        }
        let new_state_key = self.current_state_key();
        if new_state_key != state_key {
            {
                let old_state = self.states.get_mut(&state_key).unwrap();
                old_state.exit(&mut self.app_context);
            }
            let new_state = self.states.get_mut(&new_state_key).unwrap();
            new_state.enter(&mut self.app_context);
        }
    }
}

fn new_fsm(program_name: String, config: ProgramConfig) -> Machine<ProgramState, Program> {
    let program = Program::new(program_name, config.clone());
    let init_state = if config.autostart() {
        ProgramState::Starting
    } else {
        ProgramState::Stopped
    };
    let stopped: Box<dyn State<ProgramState, Program>> = Box::new(Stopped::default());
    let exited: Box<dyn State<ProgramState, Program>> = Box::new(Exited::default());
    let backoff: Box<dyn State<ProgramState, Program>> = Box::new(Backoff::default());
    let starting: Box<dyn State<ProgramState, Program>> = Box::new(Starting::default());
    let running: Box<dyn State<ProgramState, Program>> = Box::new(Running::default());
    return Machine::<ProgramState, Program>::new(
        program,
        init_state,
        HashMap::from([
            (ProgramState::Stopped, stopped),
            (ProgramState::Exited, exited),
            (ProgramState::Backoff, backoff),
            (ProgramState::Starting, starting),
            (ProgramState::Running, running),
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

#[derive(Clone, Eq, Hash, PartialEq)]
enum ProgramState {
    Stopped,
    Exited,
    Backoff,
    Starting,
    Running,
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

impl State<ProgramState, Program> for Stopped {
    fn update(&mut self, _context: &mut dyn Context<ProgramState>, _program: &mut Program) {}

    fn enter(&mut self, program: &mut Program) {
        program.num_restarts = 0u32;
        println!("{} stopped", program.name);
    }
}

impl State<ProgramState, Program> for Exited {
    fn update(&mut self, _context: &mut dyn Context<ProgramState>, _program: &mut Program) {}

    fn enter(&mut self, program: &mut Program) {
        program.num_restarts = 0u32;
        println!("{} exited", program.name);
    }
}

impl State<ProgramState, Program> for Backoff {
    fn update(&mut self, context: &mut dyn Context<ProgramState>, program: &mut Program) {
        let now = std::time::Instant::now();
        if (now - self.enter_time.unwrap())
            >= std::time::Duration::from_secs(program.config.backoff_delay() as u64)
        {
            context.transition(ProgramState::Starting);
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

impl State<ProgramState, Program> for Starting {
    fn update(&mut self, context: &mut dyn Context<ProgramState>, program: &mut Program) {
        if program.is_running() {
            context.transition(ProgramState::Running);
        } else {
            transition_to_backoff_or_exited(context, program);
        }
    }

    fn enter(&mut self, program: &mut Program) {
        println!("{} starting", program.name);
        if let Err(error) = program.start() {
            println!("{} spawn error: {error}", program.name);
        }
    }
}

impl State<ProgramState, Program> for Running {
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
        } else {
            panic!("Child proc is None in Running state");
        }
    }

    fn enter(&mut self, program: &mut Program) {
        program.num_restarts = 0u32;
        println!("{} running", program.name);
    }
}

#[derive(Debug, Default)]
pub struct ChaydServiceServerImpl {}

#[tonic::async_trait]
impl ChaydService for ChaydServiceServerImpl {
    type GetStatusStream = Pin<
        Box<
            dyn futures_core::Stream<Item = Result<ChaydServiceGetStatusResponse, Status>>
                + Send
                + 'static,
        >,
    >;

    async fn get_health(
        &self,
        _request: Request<ChaydServiceGetHealthRequest>,
    ) -> Result<Response<ChaydServiceGetHealthResponse>, Status> {
        println!("Received GetHealth request");
        let response = ChaydServiceGetHealthResponse {};
        Ok(Response::new(response))
    }

    async fn get_status(
        &self,
        _request: Request<ChaydServiceGetStatusRequest>,
    ) -> Result<Response<Self::GetStatusStream>, Status> {
        println!("Opening GetStatus request");
        let output = async_stream::try_stream! {
            let mut wait_interval = tokio::time::interval(tokio::time::Duration::from_secs(1));
            loop {
                let response = ChaydServiceGetStatusResponse {
                    program_statuses: vec![],
                };
                yield response;
                wait_interval.tick().await;
            }
        };
        Ok(Response::new(Box::pin(output) as Self::GetStatusStream))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let config = read_config(&args.config_path).unwrap_or_else(|error| {
        println!("Error parsing toml file: {}", error);
        std::process::exit(1);
    });

    let mut program_fsms: Vec<Machine<ProgramState, Program>> = config
        .programs
        .iter()
        .map(|(program_name, program_config)| new_fsm(program_name.clone(), program_config.clone()))
        .collect();

    let chayd_addr = "[::1]:50051".parse()?;
    let chayd_server = ChaydServiceServerImpl::default();

    tokio::spawn(
        Server::builder()
            .add_service(ChaydServiceServer::new(chayd_server))
            .serve(chayd_addr),
    );

    let mut fsm_update_interval = tokio::time::interval(tokio::time::Duration::from_secs(1));
    loop {
        fsm_update_interval.tick().await;
        for program_fsm in &mut program_fsms {
            program_fsm.update();
        }
    }
}
