use clap::Parser;
use std::collections::BTreeMap;
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
    should_stop: bool,
    stopped: bool,
}

impl Program {
    fn new(name: String, config: ProgramConfig) -> Program {
        Program {
            name,
            config,
            child_proc: None,
            should_stop: false,
            stopped: true,
        }
    }
}

fn read_config(config_path: &std::path::PathBuf) -> std::io::Result<Config> {
    let content = std::fs::read_to_string(config_path)?;
    Ok(toml::from_str(&content)?)
}

fn update_program_state(program: &mut Program) {
    if let Some(child_proc) = &mut program.child_proc {
        match child_proc.try_wait() {
            Ok(Some(status)) => {
                if program.should_stop {
                    println!("{}\tStopped ({status})", &program.name);
                } else {
                    println!("{}\tExited ({status})", &program.name);
                }
            }
            Ok(None) => {
                if program.should_stop {
                    println!("{}\tStopping", &program.name);
                } else {
                    println!("{}\tRunning", &program.name);
                }
            }
            Err(e) => {
                println!("Starting ({e})");
            }
        }
    } else {
        let mut command = std::process::Command::new(&program.config.command);
        if let Some(args) = &program.config.args {
            for arg in args {
                println!("arg: {}", arg);
            }
            command.args(args);
        }
        match command.spawn() {
            Ok(child_proc) => {
                program.child_proc.replace(child_proc);
                println!("Starting");
            }
            Err(error) => {
                println!("{}\tExited (Spawn Error: {})", &program.name, error);
            }
        }
    }
}

fn main() {
    let args = Args::parse();
    let config = read_config(&args.config_path).unwrap_or_else(|error| {
        println!("Error parsing toml file: {}", error);
        std::process::exit(1);
    });
    let mut programs: Vec<Program> = config
        .programs
        .iter()
        .map(|(name, config)| Program::new(name.clone(), config.clone()))
        .collect();
    println!("Programs: {:?}", programs);
    loop {
        println!("------------------------------------");
        for program in &mut programs {
            update_program_state(program);
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
