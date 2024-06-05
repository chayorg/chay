use nix::sys::signal::Signal;
use nix::unistd::Pid;
use std::os::fd::{AsRawFd, FromRawFd};

#[derive(Debug)]
pub struct Program {
    pub name: String,
    pub command: String,
    pub args: Option<Vec<String>>,
    pub child_proc: Option<std::process::Child>,
}

impl Program {
    pub fn new(name: String, command: String, args: Option<Vec<String>>) -> Program {
        Program {
            name,
            command,
            args,
            child_proc: None,
        }
    }

    pub fn start(
        &mut self,
        pipe_stdin: bool,
        parent_proc: Option<&mut std::process::Child>,
    ) -> std::io::Result<()> {
        self.reset_child_proc();
        let mut command = std::process::Command::new(&self.command);
        if let Some(args) = &self.args {
            command.args(args);
        }
        if pipe_stdin {
            command.stdin(std::process::Stdio::piped());
        }
        if let Some(parent_proc) = parent_proc {
            // NOTE: This will panic if parent_proc's stdin was not piped.
            let parent_stdin = parent_proc.stdin.take().unwrap();
            command.stderr(unsafe {
                std::process::Stdio::from_raw_fd(
                    nix::unistd::dup(parent_stdin.as_raw_fd()).unwrap(),
                )
            });
            command.stdout(parent_stdin);
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
            panic!("Program::send_signal called while not running");
        });
        let pid = Pid::from_raw(child_proc.id() as i32);
        nix::sys::signal::kill(pid, signal)
    }

    pub fn reset_child_proc(&mut self) {
        self.reap();
        self.child_proc = None;
    }

    pub fn is_running(&mut self) -> bool {
        if let Some(child_proc) = &mut self.child_proc {
            return match child_proc.try_wait() {
                // If the ExitStatus is None, that means the exit status is not yet ready. This
                // should only happen if the process is still running.
                Ok(None) => true,
                Ok(Some(_)) | Err(_) => false,
            };
        }
        false
    }

    pub fn reap(&mut self) {
        if let Some(child_proc) = &mut self.child_proc {
            match child_proc.try_wait() {
                Ok(None) => {
                    panic!("Reaped running program: {}", self.name);
                }
                Ok(Some(_)) | Err(_) => (),
            };
        }
    }
}

impl Drop for Program {
    fn drop(&mut self) {
        // This is not ideal, but it will at least ensure tha tno matter what we always kill child
        // processes when we exit in the case of a panic, etc.
        if let Some(child_proc) = &mut self.child_proc {
            println!("Force-killing child proc on drop: {}", self.name);
            match child_proc.kill() {
                Ok(_) => (),
                Err(_) => (),
            }
        }
    }
}
