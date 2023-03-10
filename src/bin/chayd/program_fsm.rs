use crate::program_context::ProgramContext;
use nix::sys::signal::Signal;
use std::collections::HashMap;

pub type ProgramFsm = chay::fsm::Machine<ProgramState, ProgramContext, ProgramEvent>;

pub fn new_program_fsm(
    program_name: String,
    config: &crate::config::RenderedProgramConfig,
) -> ProgramFsm {
    let program_ctx = ProgramContext::new(&program_name, config.clone());
    let init_state = if config.autostart() {
        ProgramState::Starting
    } else {
        ProgramState::Stopped
    };
    let stopped: Box<dyn chay::fsm::State<ProgramState, ProgramContext, ProgramEvent>> =
        Box::new(Stopped::default());
    let exited: Box<dyn chay::fsm::State<ProgramState, ProgramContext, ProgramEvent>> =
        Box::new(Exited::default());
    let backoff: Box<dyn chay::fsm::State<ProgramState, ProgramContext, ProgramEvent>> =
        Box::new(Backoff::default());
    let starting: Box<dyn chay::fsm::State<ProgramState, ProgramContext, ProgramEvent>> =
        Box::new(Starting::default());
    let running: Box<dyn chay::fsm::State<ProgramState, ProgramContext, ProgramEvent>> =
        Box::new(Running::default());
    let stopping: Box<dyn chay::fsm::State<ProgramState, ProgramContext, ProgramEvent>> =
        Box::new(Stopping::default());
    return ProgramFsm::new(
        program_ctx,
        init_state,
        HashMap::from([
            (ProgramState::Stopped, stopped),
            (ProgramState::Exited, exited),
            (ProgramState::Backoff, backoff),
            (ProgramState::Starting, starting),
            (ProgramState::Running, running),
            (ProgramState::Stopping, stopping),
        ]),
    );
}

pub enum ProgramEvent {
    Start,
    Stop,
    Restart,
}

#[derive(Clone, Eq, Hash, PartialEq)]
pub enum ProgramState {
    Stopped,
    Exited,
    Backoff,
    Starting,
    Running,
    Stopping,
}

#[derive(Default)]
pub struct Stopped {}

#[derive(Default)]
pub struct Exited {}

#[derive(Default)]
pub struct Backoff {
    enter_time: Option<std::time::Instant>,
}

#[derive(Default)]
pub struct Starting {}

#[derive(Default)]
pub struct Running {}

#[derive(Default)]
pub struct Stopping {
    sigterm_time: Option<std::time::Instant>,
}

impl chay::fsm::State<ProgramState, ProgramContext, ProgramEvent> for Stopped {
    fn react(
        &mut self,
        event: &ProgramEvent,
        context: &mut dyn chay::fsm::Context<ProgramState>,
        _program_ctx: &mut ProgramContext,
    ) -> chay::fsm::MachineResult {
        match event {
            ProgramEvent::Start => {
                context.transition(ProgramState::Starting);
                chay::fsm::MachineResult::Ok(None)
            }
            ProgramEvent::Stop => chay::fsm::MachineResult::Ok(Some("Already stopped".to_string())),
            ProgramEvent::Restart => {
                context.transition(ProgramState::Starting);
                chay::fsm::MachineResult::Ok(Some("Wasn't running (was stopped)".to_string()))
            }
        }
    }

    fn enter(&mut self, program_ctx: &mut ProgramContext) {
        program_ctx.num_restarts = 0u32;
        program_ctx.reset();
        println!("{} stopped", program_ctx.name);
    }
}

impl chay::fsm::State<ProgramState, ProgramContext, ProgramEvent> for Exited {
    fn react(
        &mut self,
        event: &ProgramEvent,
        context: &mut dyn chay::fsm::Context<ProgramState>,
        _program_ctx: &mut ProgramContext,
    ) -> chay::fsm::MachineResult {
        match event {
            ProgramEvent::Start => {
                context.transition(ProgramState::Starting);
                chay::fsm::MachineResult::Ok(None)
            }
            ProgramEvent::Stop => {
                chay::fsm::MachineResult::Ok(Some("Already stopped (exited)".to_string()))
            }
            ProgramEvent::Restart => {
                context.transition(ProgramState::Starting);
                chay::fsm::MachineResult::Ok(Some("Wasn't running (was exited)".to_string()))
            }
        }
    }

    fn enter(&mut self, program_ctx: &mut ProgramContext) {
        program_ctx.num_restarts = 0u32;
        program_ctx.reset();
        println!("{} exited", program_ctx.name);
    }
}

impl chay::fsm::State<ProgramState, ProgramContext, ProgramEvent> for Backoff {
    fn update(
        &mut self,
        context: &mut dyn chay::fsm::Context<ProgramState>,
        program_ctx: &mut ProgramContext,
    ) {
        let now = std::time::Instant::now();
        if (now - self.enter_time.unwrap())
            >= std::time::Duration::from_secs(program_ctx.config.backoff_delay() as u64)
        {
            context.transition(ProgramState::Starting);
        }
    }

    fn react(
        &mut self,
        event: &ProgramEvent,
        context: &mut dyn chay::fsm::Context<ProgramState>,
        program_ctx: &mut ProgramContext,
    ) -> chay::fsm::MachineResult {
        match event {
            ProgramEvent::Start => {
                // Reset num_restarts since the client explicitly told us to start again.
                program_ctx.num_restarts = 0u32;
                context.transition(ProgramState::Starting);
                chay::fsm::MachineResult::Ok(Some("Already starting (was backoff)".to_string()))
            }
            ProgramEvent::Stop => {
                context.transition(ProgramState::Stopping);
                chay::fsm::MachineResult::Ok(None)
            }
            ProgramEvent::Restart => {
                // Reset num_restarts since the client explicitly told us to start again.
                program_ctx.num_restarts = 0u32;
                context.transition(ProgramState::Starting);
                chay::fsm::MachineResult::Ok(Some("Wasn't running (was backoff)".to_string()))
            }
        }
    }

    fn enter(&mut self, program_ctx: &mut ProgramContext) {
        println!(
            "{} backoff (delay: {})",
            program_ctx.name,
            program_ctx.config.backoff_delay()
        );
        program_ctx.num_restarts += 1u32;
        self.enter_time.replace(std::time::Instant::now());
    }
}

impl chay::fsm::State<ProgramState, ProgramContext, ProgramEvent> for Starting {
    fn update(
        &mut self,
        context: &mut dyn chay::fsm::Context<ProgramState>,
        program_ctx: &mut ProgramContext,
    ) {
        if program_ctx.program.is_running() {
            context.transition(ProgramState::Running);
        } else {
            transition_to_backoff_or_exited(context, program_ctx);
        }
    }

    fn react(
        &mut self,
        event: &ProgramEvent,
        context: &mut dyn chay::fsm::Context<ProgramState>,
        program_ctx: &mut ProgramContext,
    ) -> chay::fsm::MachineResult {
        match event {
            ProgramEvent::Start => {
                // Reset num_restarts since the client explicitly told us to start again.
                program_ctx.num_restarts = 0u32;
                context.transition(ProgramState::Starting);
                chay::fsm::MachineResult::Ok(Some("Already starting".to_string()))
            }
            ProgramEvent::Stop => {
                context.transition(ProgramState::Stopping);
                chay::fsm::MachineResult::Ok(None)
            }
            ProgramEvent::Restart => {
                // Reset num_restarts since the client explicitly told us to start again.
                program_ctx.num_restarts = 0u32;
                chay::fsm::MachineResult::Ok(Some(
                    "Already starting (resetting backoff counter)".to_string(),
                ))
            }
        }
    }

    fn enter(&mut self, program_ctx: &mut ProgramContext) {
        println!("{} starting", program_ctx.name);
        if let Err(error) = program_ctx.program.start() {
            println!("{} spawn error: {error}", program_ctx.name);
        }
    }
}

impl chay::fsm::State<ProgramState, ProgramContext, ProgramEvent> for Running {
    fn update(
        &mut self,
        context: &mut dyn chay::fsm::Context<ProgramState>,
        program_ctx: &mut ProgramContext,
    ) {
        if !program_ctx.program.is_running() {
            transition_to_backoff_or_exited(context, program_ctx);
        }
    }

    fn react(
        &mut self,
        event: &ProgramEvent,
        context: &mut dyn chay::fsm::Context<ProgramState>,
        program_ctx: &mut ProgramContext,
    ) -> chay::fsm::MachineResult {
        match event {
            ProgramEvent::Start => {
                chay::fsm::MachineResult::Ok(Some("Already running".to_string()))
            }
            ProgramEvent::Stop => {
                program_ctx.should_restart = false;
                context.transition(ProgramState::Stopping);
                chay::fsm::MachineResult::Ok(None)
            }
            ProgramEvent::Restart => {
                program_ctx.should_restart = true;
                chay::fsm::MachineResult::Ok(None)
            }
        }
    }

    fn enter(&mut self, program_ctx: &mut ProgramContext) {
        program_ctx.num_restarts = 0u32;
        println!("{} running", program_ctx.name);
    }
}

impl Stopping {
    fn kill_program_and_stop(
        program_ctx: &mut ProgramContext,
        context: &mut dyn chay::fsm::Context<ProgramState>,
    ) {
        println!("Sending SIGKILL to program {}", program_ctx.name);
        match program_ctx.program.send_signal(Signal::SIGKILL) {
            Ok(_) => {}
            Err(e) => {
                println!(
                    "Could not send SIGKILL to program {}: {:?}",
                    program_ctx.name, e
                );
            }
        }
        // Always transition to Stopped, even if sending SIGKILL failed. Presumably that
        // would only happen if the program has already terminated somehow. I am not
        // sure if this is actually possible.
        Stopping::transition_to_stopped_or_restart(program_ctx.should_restart, context);
    }

    fn transition_to_stopped_or_restart(
        should_restart: bool,
        context: &mut dyn chay::fsm::Context<ProgramState>,
    ) {
        if should_restart {
            context.transition(ProgramState::Starting);
        } else {
            context.transition(ProgramState::Stopped);
        }
    }
}

impl chay::fsm::State<ProgramState, ProgramContext, ProgramEvent> for Stopping {
    fn update(
        &mut self,
        context: &mut dyn chay::fsm::Context<ProgramState>,
        program_ctx: &mut ProgramContext,
    ) {
        if !program_ctx.program.is_running() {
            Stopping::transition_to_stopped_or_restart(program_ctx.should_restart, context);
            return;
        }
        if let Some(sigterm_time) = self.sigterm_time {
            // We already sent SIGTERM in a previous update. Send SIGKILL if it
            // doesn't shut down within a reasonable timeout.
            let now = std::time::Instant::now();
            let sigkill_timeout =
                std::time::Duration::from_secs(program_ctx.config.sigkill_delay() as u64);
            if (now - sigterm_time) >= sigkill_timeout {
                Stopping::kill_program_and_stop(program_ctx, context);
            }
        } else {
            // We haven't sent SIGTERM yet, so do that now.
            self.sigterm_time = Some(std::time::Instant::now());
            match program_ctx.program.send_signal(Signal::SIGTERM) {
                Ok(_) => {}
                Err(error) => {
                    println!(
                        "Could not send SIGTERM to program {}: {:?}",
                        program_ctx.name, error
                    );
                    Stopping::kill_program_and_stop(program_ctx, context);
                }
            }
        }
    }

    fn react(
        &mut self,
        event: &ProgramEvent,
        _context: &mut dyn chay::fsm::Context<ProgramState>,
        program_ctx: &mut ProgramContext,
    ) -> chay::fsm::MachineResult {
        match event {
            ProgramEvent::Start => {
                chay::fsm::MachineResult::Err("Cannot start while stopping".to_string())
            }
            ProgramEvent::Stop => {
                program_ctx.should_restart = false;
                chay::fsm::MachineResult::Ok(Some("Already stopping".to_string()))
            }
            ProgramEvent::Restart => {
                program_ctx.should_restart = true;
                chay::fsm::MachineResult::Ok(Some("Will restart after stopping".to_string()))
            }
        }
    }

    fn enter(&mut self, program_ctx: &mut ProgramContext) {
        self.sigterm_time = None;
        program_ctx.num_restarts = 0u32;
        println!("{} stopping", program_ctx.name);
    }

    fn exit(&mut self, program_ctx: &mut ProgramContext) {
        program_ctx.should_restart = false;
    }
}

fn transition_to_backoff_or_exited(
    context: &mut dyn chay::fsm::Context<ProgramState>,
    program_ctx: &mut ProgramContext,
) {
    if program_ctx.config.autorestart()
        && program_ctx.num_restarts < program_ctx.config.num_restart_attempts()
    {
        context.transition(ProgramState::Backoff);
    } else {
        context.transition(ProgramState::Exited);
    }
}
