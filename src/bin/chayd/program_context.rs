use crate::program::Program;

/// Runs a program and all associated sidecar programs (i.e. logger if one is configured).
pub struct ProgramContext {
    pub name: String,
    pub config: crate::config::RenderedProgramConfig,
    pub program: Program,
    pub pre_command: Option<Program>,
    pub logger: Option<Program>,
    pub logger_pre_command: Option<Program>,
    pub num_restarts: u32,
    pub should_restart: bool,
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

impl ProgramContext {
    pub fn new(name: &str, config: crate::config::RenderedProgramConfig) -> Self {
        let program = Program::new(name.to_string(), config.command(), config.args());
        let logger_pre_command_program = if let Some(logger_config) = &config.logger {
            if let Some(logger_pre_command_config) = &logger_config.pre_command {
                Some(Program::new(
                    logger_pre_command_name(name),
                    logger_pre_command_config.command.clone(),
                    logger_pre_command_config.args.clone(),
                ))
            } else {
                None
            }
        } else {
            None
        };
        let logger_program = if let Some(logger_config) = &config.logger {
            Some(Program::new(
                logger_name(name),
                logger_config.command.clone(),
                logger_config.args.clone(),
            ))
        } else {
            None
        };
        let pre_command_program = if let Some(pre_command_config) = &config.program.pre_command {
            Some(Program::new(
                pre_command_name(name),
                pre_command_config.command.clone(),
                pre_command_config.args.clone(),
            ))
        } else {
            None
        };
        Self {
            name: name.to_string(),
            config,
            program,
            logger: logger_program,
            logger_pre_command: logger_pre_command_program,
            pre_command: pre_command_program,
            num_restarts: 0u32,
            should_restart: false,
        }
    }

    pub fn name(&self) -> String {
        self.name.clone()
    }

    pub fn all_programs_are_running(&mut self) -> bool {
        if let Some(logger) = &mut self.logger {
            if !logger.is_running() {
                return false;
            }
        }
        self.program.is_running()
    }

    pub fn all_programs_are_stopped(&mut self) -> bool {
        if self.program.is_running() {
            return false;
        }
        if let Some(logger) = &mut self.logger {
            if logger.is_running() {
                return false;
            }
        }
        true
    }

    pub fn reset(&mut self) {
        self.program.reset_child_proc();
        if let Some(logger) = &mut self.logger {
            logger.reset_child_proc();
        }
    }
}
