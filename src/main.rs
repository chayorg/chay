use clap::Parser;
use std::collections::{BTreeMap, HashMap};
use toml;

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
    child_proc: Option<std::process::Child>,
}

impl Program {
    fn new(name: String, config: ProgramConfig) -> Program {
        Program {
            name,
            config,
            child_proc: None,
        }
    }

    fn start(&mut self) -> std::io::Result<()> {
        let mut command = std::process::Command::new(&self.config.command);
        if let Some(args) = &self.config.args {
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
    let init_state = if config.autostart.unwrap_or(true) {
        ProgramState::Starting
    } else {
        ProgramState::Stopped
    };
    let stopped: Box<dyn State<ProgramState, Program>> = Box::new(Stopped::default());
    let exited: Box<dyn State<ProgramState, Program>> = Box::new(Exited::default());
    let starting: Box<dyn State<ProgramState, Program>> = Box::new(Starting::default());
    let running: Box<dyn State<ProgramState, Program>> = Box::new(Running::default());
    return Machine::<ProgramState, Program>::new(
        program,
        init_state,
        HashMap::from([
            (ProgramState::Stopped, stopped),
            (ProgramState::Exited, exited),
            (ProgramState::Starting, starting),
            (ProgramState::Running, running),
        ]),
    );
}

#[derive(Clone, Eq, Hash, PartialEq)]
enum ProgramState {
    Stopped,
    Exited,
    Starting,
    Running,
}

#[derive(Default)]
struct Stopped {}
#[derive(Default)]
struct Exited {}
#[derive(Default)]
struct Starting {}
#[derive(Default)]
struct Running {}

impl State<ProgramState, Program> for Stopped {
    fn update(&mut self, _context: &mut dyn Context<ProgramState>, _program: &mut Program) {}

    fn enter(&mut self, program: &mut Program) {
        println!("{} stopped", program.name);
    }
}

impl State<ProgramState, Program> for Exited {
    fn update(&mut self, _context: &mut dyn Context<ProgramState>, _program: &mut Program) {}

    fn enter(&mut self, program: &mut Program) {
        println!("{} exited", program.name);
    }
}

impl State<ProgramState, Program> for Starting {
    fn update(&mut self, context: &mut dyn Context<ProgramState>, program: &mut Program) {
        if let Some(child_proc) = &mut program.child_proc {
            match child_proc.try_wait() {
                Ok(Some(_)) => {
                    context.transition(ProgramState::Exited);
                }
                Ok(None) => {
                    context.transition(ProgramState::Running);
                }
                Err(_) => {
                    context.transition(ProgramState::Exited);
                }
            }
        } else {
            // This will happen when there was a spawn error trying to start the process in the
            // enter callback.
            context.transition(ProgramState::Exited);
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
                Ok(Some(_)) => {
                    context.transition(ProgramState::Exited);
                }
                Ok(None) => {
                    // Running. Nothing to do.
                }
                Err(_) => {
                    context.transition(ProgramState::Exited);
                }
            }
        } else {
            panic!("Child proc is none in Running state");
        }
    }

    fn enter(&mut self, program: &mut Program) {
        println!("{} running", program.name);
    }
}

fn main() {
    let args = Args::parse();
    let config = read_config(&args.config_path).unwrap_or_else(|error| {
        println!("Error parsing toml file: {}", error);
        std::process::exit(1);
    });
    let mut program_fsms: Vec<Machine<ProgramState, Program>> = config
        .programs
        .iter()
        .map(|(name, config)| new_fsm(name.clone(), config.clone()))
        .collect();
    loop {
        for program_fsm in &mut program_fsms {
            program_fsm.update();
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
