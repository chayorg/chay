use nix::sys::signal::Signal;
use nix::unistd::Pid;

#[derive(Debug)]
pub struct Program {
    pub name: String,
    pub command: String,
    pub args: Option<Vec<String>>,
    pub num_restarts: u32,
    pub child_proc: Option<std::process::Child>,
    pub should_restart: bool,
}

impl Program {
    pub fn new(name: String, command: String, args: Option<Vec<String>>) -> Program {
        Program {
            name,
            command,
            args,
            num_restarts: 0u32,
            child_proc: None,
            should_restart: false,
        }
    }

    pub fn start(&mut self) -> std::io::Result<()> {
        self.reset_child_proc();
        let mut command = std::process::Command::new(&self.command);
        if let Some(args) = &self.args {
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

    pub fn send_signal(&self, signal: Signal) -> nix::Result<()> {
        let child_proc = self.child_proc.as_ref().unwrap_or_else(|| {
            panic!("Program::send_sigterm called while not running");
        });
        let pid = Pid::from_raw(child_proc.id() as i32);
        nix::sys::signal::kill(pid, signal)
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
