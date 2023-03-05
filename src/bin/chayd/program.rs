#[derive(Debug)]
pub struct Program {
    pub name: String,
    pub config: crate::config::RenderedProgramConfig,
    pub num_restarts: u32,
    pub child_proc: Option<std::process::Child>,
    pub should_restart: bool,
}

impl Program {
    pub fn new(name: String, config: crate::config::RenderedProgramConfig) -> Program {
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
