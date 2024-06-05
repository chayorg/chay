use crate::program_context::ProgramContext;
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
    let exiting: Box<dyn chay::fsm::State<ProgramState, ProgramContext, ProgramEvent>> =
        Box::new(Exiting::default());
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
            (ProgramState::Exiting, exiting),
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
    Exiting,
}

#[derive(Default)]
pub struct Stopped {}

#[derive(Default)]
pub struct Exited {}

#[derive(Default)]
pub struct Backoff {
    enter_time: Option<std::time::Instant>,
    skip_backoff_delay: bool,
}

#[derive(Default)]
pub struct Starting {}

#[derive(Default)]
pub struct Running {}

#[derive(Default)]
pub struct Stopping {}

#[derive(Default)]
pub struct Exiting {}

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
        if !program_ctx.all_programs_are_stopped() {
            // Ensure everything is stopped from the previous running state before we restart.
            program_ctx.send_sigterm_or_sigkill_signal_to_all_running_programs();
            if !program_ctx.all_programs_are_stopped() {
                return;
            }
        }
        if self.skip_backoff_delay {
            context.transition(ProgramState::Starting);
            return;
        }
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
                if program_ctx.all_programs_are_stopped() {
                    context.transition(ProgramState::Starting);
                    return chay::fsm::MachineResult::Ok(Some(
                        "Already starting (was backoff)".to_string(),
                    ));
                }
                self.skip_backoff_delay = true;
                chay::fsm::MachineResult::Ok(Some("Will start after backoff cleanup".to_string()))
            }
            ProgramEvent::Stop => {
                context.transition(ProgramState::Stopping);
                chay::fsm::MachineResult::Ok(None)
            }
            ProgramEvent::Restart => {
                // Reset num_restarts since the client explicitly told us to start again.
                program_ctx.num_restarts = 0u32;
                if program_ctx.all_programs_are_stopped() {
                    context.transition(ProgramState::Starting);
                    return chay::fsm::MachineResult::Ok(Some(
                        "Wasn't running (was backoff)".to_string(),
                    ));
                }
                self.skip_backoff_delay = true;
                chay::fsm::MachineResult::Ok(Some("Will restart after backoff cleanup".to_string()))
            }
        }
    }

    fn enter(&mut self, program_ctx: &mut ProgramContext) {
        println!(
            "{} backoff (delay: {})",
            program_ctx.name,
            program_ctx.config.backoff_delay()
        );
        self.skip_backoff_delay = false;
        program_ctx.sigterm_time = None;
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
        if program_ctx.all_programs_are_running() {
            context.transition(ProgramState::Running);
        } else {
            transition_to_backoff_or_exiting(context, program_ctx);
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
        if let Some(logger) = &mut program_ctx.logger {
            if !logger.is_running() {
                if let Err(error) = logger.start(true, None) {
                    println!("{} spawn error: {error}", logger.name);
                    return;
                }
            }
            if let Err(error) = program_ctx
                .program
                .start(false, Some(&mut logger.child_proc.as_mut().unwrap()))
            {
                println!("{} spawn error: {error}", program_ctx.name);
            }
        } else {
            if let Err(error) = program_ctx.program.start(false, None) {
                println!("{} spawn error: {error}", program_ctx.name);
            }
        }
    }
}

impl chay::fsm::State<ProgramState, ProgramContext, ProgramEvent> for Running {
    fn update(
        &mut self,
        context: &mut dyn chay::fsm::Context<ProgramState>,
        program_ctx: &mut ProgramContext,
    ) {
        if !program_ctx.all_programs_are_running() {
            transition_to_backoff_or_exiting(context, program_ctx);
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
                context.transition(ProgramState::Stopping);
                chay::fsm::MachineResult::Ok(None)
            }
        }
    }

    fn enter(&mut self, program_ctx: &mut ProgramContext) {
        program_ctx.num_restarts = 0u32;
        println!("{} running", program_ctx.name);
    }
}

impl chay::fsm::State<ProgramState, ProgramContext, ProgramEvent> for Stopping {
    fn update(
        &mut self,
        context: &mut dyn chay::fsm::Context<ProgramState>,
        program_ctx: &mut ProgramContext,
    ) {
        if program_ctx.all_programs_are_stopped() {
            transition_to_stopped_or_restart(program_ctx.should_restart, context);
            return;
        }
        program_ctx.send_sigterm_or_sigkill_signal_to_all_running_programs();
        // Check again if everything is stopped in case we just killed everything above.
        if program_ctx.all_programs_are_stopped() {
            transition_to_stopped_or_restart(program_ctx.should_restart, context);
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
        program_ctx.sigterm_time = None;
        program_ctx.num_restarts = 0u32;
        println!("{} stopping", program_ctx.name);
    }

    fn exit(&mut self, program_ctx: &mut ProgramContext) {
        program_ctx.should_restart = false;
    }
}

impl chay::fsm::State<ProgramState, ProgramContext, ProgramEvent> for Exiting {
    fn update(
        &mut self,
        context: &mut dyn chay::fsm::Context<ProgramState>,
        program_ctx: &mut ProgramContext,
    ) {
        if program_ctx.all_programs_are_stopped() {
            transition_to_exited_or_restart(program_ctx.should_restart, context);
            return;
        }
        program_ctx.send_sigterm_or_sigkill_signal_to_all_running_programs();
        // Check again if everything is stopped in case we just killed everything above.
        if program_ctx.all_programs_are_stopped() {
            transition_to_exited_or_restart(program_ctx.should_restart, context);
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
                chay::fsm::MachineResult::Err("Cannot start while exiting".to_string())
            }
            ProgramEvent::Stop => {
                program_ctx.should_restart = false;
                chay::fsm::MachineResult::Ok(Some("Already stopping (exiting)".to_string()))
            }
            ProgramEvent::Restart => {
                program_ctx.should_restart = true;
                chay::fsm::MachineResult::Ok(Some("Will restart after exiting".to_string()))
            }
        }
    }

    fn enter(&mut self, program_ctx: &mut ProgramContext) {
        program_ctx.sigterm_time = None;
        program_ctx.num_restarts = 0u32;
        println!("{} exiting", program_ctx.name);
    }

    fn exit(&mut self, program_ctx: &mut ProgramContext) {
        program_ctx.should_restart = false;
    }
}

fn transition_to_backoff_or_exiting(
    context: &mut dyn chay::fsm::Context<ProgramState>,
    program_ctx: &mut ProgramContext,
) {
    if program_ctx.config.autorestart()
        && program_ctx.num_restarts < program_ctx.config.num_restart_attempts()
    {
        context.transition(ProgramState::Backoff);
    } else {
        if program_ctx.all_programs_are_stopped() {
            context.transition(ProgramState::Exited);
        } else {
            context.transition(ProgramState::Exiting);
        }
    }
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

fn transition_to_exited_or_restart(
    should_restart: bool,
    context: &mut dyn chay::fsm::Context<ProgramState>,
) {
    if should_restart {
        context.transition(ProgramState::Starting);
    } else {
        context.transition(ProgramState::Exited);
    }
}
