use crate::program::Program;
use nix::sys::signal::Signal;

pub enum PrecommandStatus {
    RUNNING,
    SUCCESS,
    ERROR,
}

#[derive(Default)]
pub struct PrecommandContext {
    pub program: Program,
    /// Consider the pre-command as failed if it doesn't finish running within this timeout.
    pub timeout: std::time::Duration,
    pub start_time: Option<std::time::Instant>,
}

impl PrecommandContext {
    fn reset(&mut self) {
        self.program.reset_child_proc();
        self.start_time = None;
    }
}

pub enum SubprogramStatus {
    STARTING,
    SUCCESS,
    ERROR,
}

#[derive(Default)]
pub struct SubprogramContext {
    pub program: Program,
    /// Don't consider the subprogram as running until it has been running for at least this long.
    pub start_wait: std::time::Duration,
    pub start_time: Option<std::time::Instant>,
}

impl SubprogramContext {
    fn reset(&mut self) {
        self.program.reset_child_proc();
        self.start_time = None;
    }
}

/// Runs a program and all associated sidecar programs (i.e. logger if one is configured).
pub struct ProgramContext {
    pub name: String,
    pub config: crate::config::RenderedProgramConfig,

    pub program: SubprogramContext,
    pub pre_command: Option<PrecommandContext>,
    pub logger: Option<SubprogramContext>,
    pub logger_pre_command: Option<PrecommandContext>,

    pub num_restarts: u32,
    pub should_restart: bool,
    pub sigterm_time: Option<std::time::Instant>,
}

fn logger_pre_command_name(program_name: &str) -> String {
    format!("{program_name}-logger-pre-command")
}

fn logger_name(program_name: &str) -> String {
    format!("{program_name}-logger")
}

fn pre_command_name(program_name: &str) -> String {
    format!("{program_name}-pre-command")
}

fn signal_to_str(signal: Signal) -> String {
    match signal {
        Signal::SIGKILL => "SIGKILL".to_string(),
        Signal::SIGTERM => "SIGTERM".to_string(),
        _ => panic!("Unknown signal"),
    }
}

fn send_signal_to_program_if_running(program: &mut Program, signal: Signal) {
    if program.is_running() {
        match program.send_signal(signal) {
            Ok(_) => {}
            Err(error) => {
                println!(
                    "Could not send {} to program {}: {:?}",
                    signal_to_str(signal),
                    program.name,
                    error
                );
            }
        }
    }
}

impl ProgramContext {
    pub fn new(name: &str, config: crate::config::RenderedProgramConfig) -> Self {
        let program = SubprogramContext {
            program: Program::new(
                name.to_string(),
                config.program.command.clone(),
                config.program.args.clone(),
            ),
            start_wait: std::time::Duration::from_secs(config.program.start_wait_secs as u64),
            start_time: None,
        };
        let pre_command = if let Some(pre_command_config) = &config.program.pre_command {
            Some(PrecommandContext {
                program: Program::new(
                    pre_command_name(name),
                    pre_command_config.command.clone(),
                    pre_command_config.args.clone(),
                ),
                timeout: std::time::Duration::from_secs(pre_command_config.timeout_secs as u64),
                start_time: None,
            })
        } else {
            None
        };
        let logger = if let Some(logger_config) = &config.logger {
            Some(SubprogramContext {
                program: Program::new(
                    logger_name(name),
                    logger_config.command.clone(),
                    logger_config.args.clone(),
                ),
                start_wait: std::time::Duration::from_secs(logger_config.start_wait_secs as u64),
                start_time: None,
            })
        } else {
            None
        };
        let logger_pre_command = if let Some(logger_config) = &config.logger {
            if let Some(logger_pre_command_config) = &logger_config.pre_command {
                Some(PrecommandContext {
                    program: Program::new(
                        logger_pre_command_name(name),
                        logger_pre_command_config.command.clone(),
                        logger_pre_command_config.args.clone(),
                    ),
                    timeout: std::time::Duration::from_secs(
                        logger_pre_command_config.timeout_secs as u64,
                    ),
                    start_time: None,
                })
            } else {
                None
            }
        } else {
            None
        };
        Self {
            name: name.to_string(),
            config,
            program,
            pre_command,
            logger,
            logger_pre_command,
            num_restarts: 0u32,
            should_restart: false,
            sigterm_time: None,
        }
    }

    pub fn name(&self) -> String {
        self.name.clone()
    }

    pub fn all_programs_are_running(&mut self) -> bool {
        if let Some(logger) = &mut self.logger {
            if !logger.program.is_running() {
                return false;
            }
        }
        self.program.program.is_running()
    }

    pub fn all_programs_are_stopped(&mut self) -> bool {
        if self.program.program.is_running() {
            return false;
        }
        if let Some(pre_command) = &mut self.pre_command {
            if pre_command.program.is_running() {
                return false;
            }
        }
        if let Some(logger) = &mut self.logger {
            if logger.program.is_running() {
                return false;
            }
        }
        if let Some(logger_pre_command) = &mut self.logger_pre_command {
            if logger_pre_command.program.is_running() {
                return false;
            }
        }
        true
    }

    pub fn reset(&mut self) {
        self.program.reset();
        if let Some(pre_command) = &mut self.pre_command {
            pre_command.reset();
        }
        if let Some(logger) = &mut self.logger {
            logger.reset();
        }
        if let Some(logger_pre_command) = &mut self.logger_pre_command {
            logger_pre_command.reset();
        }
        self.sigterm_time = None;
        // NOTE: Intentionally do not reset num_restarts or should_restart here. Those are reset
        // seperately during in the appropriate state transitions.
    }

    pub fn send_signal_to_all_running_programs(&mut self, signal: Signal) {
        if let Some(pre_command) = &mut self.pre_command {
            send_signal_to_program_if_running(&mut pre_command.program, signal);
        }
        if let Some(logger_pre_command) = &mut self.logger_pre_command {
            send_signal_to_program_if_running(&mut logger_pre_command.program, signal);
        }
        if let Some(logger) = &mut self.logger {
            send_signal_to_program_if_running(&mut logger.program, signal);
        }
        send_signal_to_program_if_running(&mut self.program.program, signal);
    }

    pub fn send_sigterm_or_sigkill_signal_to_all_running_programs(&mut self) {
        if let Some(sigterm_time) = self.sigterm_time {
            // We already sent SIGTERM in a previous update. Send SIGKILL if it
            // doesn't shut down within a reasonable timeout.
            let now = std::time::Instant::now();
            let sigkill_timeout =
                std::time::Duration::from_secs(self.config.sigkill_delay_secs() as u64);
            if (now - sigterm_time) >= sigkill_timeout {
                self.send_signal_to_all_running_programs(Signal::SIGKILL);
            }
        } else {
            // We haven't sent SIGTERM yet, so do that now.
            self.sigterm_time = Some(std::time::Instant::now());
            self.send_signal_to_all_running_programs(Signal::SIGTERM);
        }
    }
}
