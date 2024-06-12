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
            >= std::time::Duration::from_secs(program_ctx.config.backoff_delay_secs() as u64)
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
            "{} backoff (delay: {} secs)",
            program_ctx.name,
            program_ctx.config.backoff_delay_secs()
        );
        self.skip_backoff_delay = false;
        program_ctx.sigterm_time = None;
        program_ctx.num_restarts += 1u32;
        self.enter_time.replace(std::time::Instant::now());
    }

    fn exit(&mut self, program_ctx: &mut ProgramContext) {
        program_ctx.reset()
    }
}

impl Starting {
    /// Returns true if the pre_command has successfully finished running.
    /// If an error or timeout occurred, this function handles the state transition.
    fn check_pre_command(
        pre_command: &mut crate::program_context::PrecommandContext,
        now: std::time::Instant,
    ) -> crate::program_context::PrecommandStatus {
        if let Some(start_time) = &mut pre_command.start_time {
            match pre_command.program.exit_status_unchecked() {
                Ok(None) => {
                    // The program is still running. Check if it has timed out.
                    if now - *start_time > pre_command.timeout {
                        // The program failed to run within the given timeout.
                        println!("{} timed out", pre_command.program.name);
                        return crate::program_context::PrecommandStatus::ERROR;
                    }
                    // We are still waiting for pre_command to finish running.
                    return crate::program_context::PrecommandStatus::RUNNING;
                }
                Ok(Some(exit_status)) => {
                    if !exit_status.success() {
                        if let Some(code) = exit_status.code() {
                            println!(
                                "{} exited with non-zero code {}",
                                pre_command.program.name, code
                            );
                        } else {
                            // I don't know if this can ever happen. The documentation says
                            // that on unix this can be None if the program was terminated by
                            // a signal, which shouldn't ever happen here to my knowledge.
                            println!(
                                "{} exited with non-zero code [unknown]",
                                pre_command.program.name
                            );
                        }
                        return crate::program_context::PrecommandStatus::ERROR;
                    }
                    // The program finished running successfully!
                    return crate::program_context::PrecommandStatus::SUCCESS;
                }
                Err(error) => {
                    println!("{} exited with error {error}", pre_command.program.name);
                    return crate::program_context::PrecommandStatus::ERROR;
                }
            }
        }
        // Command has not been started yet.
        if let Err(error) = pre_command.program.start(false, None) {
            println!("{} spawn error: {error}", pre_command.program.name);
            return crate::program_context::PrecommandStatus::ERROR;
        }
        pre_command.start_time = Some(now);
        return crate::program_context::PrecommandStatus::RUNNING;
    }

    /// This function assumes the subprogram has already been started.
    fn check_subprogram(
        subprogram: &mut crate::program_context::SubprogramContext,
        now: std::time::Instant,
    ) -> crate::program_context::SubprogramStatus {
        let start_time = subprogram.start_time.as_ref().unwrap();
        match subprogram.program.exit_status_unchecked() {
            Ok(None) => {
                // The program is still running. Check if it has reached the start_wait period.
                if now - *start_time >= subprogram.start_wait {
                    return crate::program_context::SubprogramStatus::SUCCESS;
                }
                return crate::program_context::SubprogramStatus::STARTING;
            }
            Ok(Some(exit_status)) => {
                if let Some(code) = exit_status.code() {
                    println!("{} exited with code {}", subprogram.program.name, code);
                } else {
                    // I don't know if this can ever happen. The documentation says
                    // that on unix this can be None if the program was terminated by
                    // a signal, which shouldn't ever happen here to my knowledge.
                    println!("{} exited with code [unknown]", subprogram.program.name);
                }
                return crate::program_context::SubprogramStatus::ERROR;
            }
            Err(error) => {
                println!("{} exited with error {error}", subprogram.program.name);
                return crate::program_context::SubprogramStatus::ERROR;
            }
        }
    }
}

impl chay::fsm::State<ProgramState, ProgramContext, ProgramEvent> for Starting {
    fn update(
        &mut self,
        context: &mut dyn chay::fsm::Context<ProgramState>,
        program_ctx: &mut ProgramContext,
    ) {
        let now = std::time::Instant::now();

        if let Some(logger_pre_command) = &mut program_ctx.logger_pre_command {
            match Starting::check_pre_command(logger_pre_command, now.clone()) {
                crate::program_context::PrecommandStatus::RUNNING => return,
                crate::program_context::PrecommandStatus::SUCCESS => (),
                crate::program_context::PrecommandStatus::ERROR => {
                    transition_to_backoff_or_exiting(context, program_ctx);
                    return;
                }
            }
        }

        if let Some(pre_command) = &mut program_ctx.pre_command {
            match Starting::check_pre_command(pre_command, now.clone()) {
                crate::program_context::PrecommandStatus::RUNNING => return,
                crate::program_context::PrecommandStatus::SUCCESS => (),
                crate::program_context::PrecommandStatus::ERROR => {
                    transition_to_backoff_or_exiting(context, program_ctx);
                    return;
                }
            }
        }

        if let Some(logger) = &mut program_ctx.logger {
            // Start the logger if it isn't already started.
            if logger.start_time.is_none() {
                if let Err(error) = logger.program.start(true, None) {
                    println!("{} spawn error: {error}", logger.program.name);
                    transition_to_backoff_or_exiting(context, program_ctx);
                    return;
                }
                logger.start_time = Some(now);
            }

            // Start the program if it isn't already started.
            if program_ctx.program.start_time.is_none() {
                // Start the program with its stdout and stderr piped into the logger's stdin.
                if let Err(error) = program_ctx.program.program.start(
                    false,
                    Some(&mut logger.program.child_proc.as_mut().unwrap()),
                ) {
                    println!("{} spawn error: {error}", program_ctx.name);
                    transition_to_backoff_or_exiting(context, program_ctx);
                    return;
                }
                program_ctx.program.start_time = Some(now);
            }
        } else {
            // Start the program if it isn't already started.
            if program_ctx.program.start_time.is_none() {
                if let Err(error) = program_ctx.program.program.start(false, None) {
                    println!("{} spawn error: {error}", program_ctx.name);
                    transition_to_backoff_or_exiting(context, program_ctx);
                    return;
                }
                program_ctx.program.start_time = Some(now);
            }
        }

        // Wait for the program to start successfully.
        match Starting::check_subprogram(&mut program_ctx.program, now.clone()) {
            crate::program_context::SubprogramStatus::STARTING => return,
            crate::program_context::SubprogramStatus::SUCCESS => (),
            crate::program_context::SubprogramStatus::ERROR => {
                transition_to_backoff_or_exiting(context, program_ctx);
                return;
            }
        }

        if let Some(logger) = &mut program_ctx.logger {
            // Wait for the logger to start successfully.
            // NOTE: Check this after the program because the logger will often exit if the program
            // fails to start. When that happens we wouldn't get a very helpful error message. It'd
            // say the logger stopped running, rather than showing that the program exited.
            match Starting::check_subprogram(logger, now.clone()) {
                crate::program_context::SubprogramStatus::STARTING => return,
                crate::program_context::SubprogramStatus::SUCCESS => (),
                crate::program_context::SubprogramStatus::ERROR => {
                    transition_to_backoff_or_exiting(context, program_ctx);
                    return;
                }
            }
        }

        // If we get here, all the pre_commands succeeded and both the logger and program are
        // running successfully!
        context.transition(ProgramState::Running);
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
                chay::fsm::MachineResult::Ok(Some("Already starting".to_string()))
            }
            ProgramEvent::Stop => {
                context.transition(ProgramState::Stopping);
                chay::fsm::MachineResult::Ok(None)
            }
            ProgramEvent::Restart => {
                // Reset num_restarts since the client explicitly told us to start again.
                program_ctx.num_restarts = 0u32;
                program_ctx.should_restart = true;
                context.transition(ProgramState::Stopping);
                chay::fsm::MachineResult::Ok(None)
            }
        }
    }

    fn enter(&mut self, program_ctx: &mut ProgramContext) {
        println!("{} starting", program_ctx.name);
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
        program_ctx.reset()
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
        program_ctx.reset()
    }
}

fn transition_to_exited_or_exiting(
    context: &mut dyn chay::fsm::Context<ProgramState>,
    program_ctx: &mut ProgramContext,
) {
    if program_ctx.all_programs_are_stopped() {
        context.transition(ProgramState::Exited);
    } else {
        context.transition(ProgramState::Exiting);
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
        transition_to_exited_or_exiting(context, program_ctx);
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
