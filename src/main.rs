use clap::Parser;
use std::collections::HashMap;
use toml;

/// Daemon to supervise a list of processes
#[derive(clap::Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the config file
    config_path: std::path::PathBuf,
}

#[derive(Debug, serde::Deserialize)]
struct ProgramConfig {
    command: String,
}

#[derive(Debug, serde::Deserialize)]
struct Config {
    programs: HashMap<String, ProgramConfig>,
}

fn read_config(config_path: &std::path::PathBuf) -> std::io::Result<Config> {
    let content = std::fs::read_to_string(config_path)?;
    Ok(toml::from_str(&content)?)
}

fn main() {
    let args = Args::parse();
    let config = read_config(&args.config_path).unwrap_or_else(|error| {
        println!("Error parsing toml file: {}", error);
        std::process::exit(1);
    });
    println!("Config: {:?}", config);
}
