use crate::program::Program;

/// Runs a program and all associated sidecar programs (i.e. logger if one is configured).
pub struct ProgramContext {
    pub name: String,
    pub config: crate::config::RenderedProgramConfig,
    pub program: Program,
    pub logger: Option<Program>,
    pub num_restarts: u32,
    pub should_restart: bool,
}

fn logger_name(program_name: &str) -> String {
    format!("{program_name}-logger")
}

impl ProgramContext {
    pub fn new(name: &str, config: crate::config::RenderedProgramConfig) -> Self {
        let program = Program::new(name.to_string(), config.command(), config.args());
        let logger_program = if let Some(logger_config) = &config.logger {
            Some(Program::new(
                logger_name(name),
                logger_config.command.clone(),
                logger_config.args.clone(),
            ))
        } else {
            None
        };
        Self {
            name: name.to_string(),
            config,
            program,
            logger: logger_program,
            num_restarts: 0u32,
            should_restart: false,
        }
    }

    pub fn name(&self) -> String {
        self.name.clone()
    }

    pub fn reset(&mut self) {
        self.program.reset_child_proc();
        if let Some(logger) = &mut self.logger {
            logger.reset_child_proc();
        }
    }
}
