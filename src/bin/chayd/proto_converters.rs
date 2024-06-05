use crate::{chay_proto, program_fsm};
use chay_proto::{
    ChaydServiceRestartResponse, ChaydServiceStartResponse, ChaydServiceStopResponse,
};
use std::collections::HashMap;

pub fn proto_from_program_state(
    program_state: program_fsm::ProgramState,
) -> chay_proto::ProgramState {
    match program_state {
        program_fsm::ProgramState::Stopped => chay_proto::ProgramState::Stopped,
        program_fsm::ProgramState::Exited => chay_proto::ProgramState::Exited,
        program_fsm::ProgramState::Backoff => chay_proto::ProgramState::Backoff,
        program_fsm::ProgramState::Starting => chay_proto::ProgramState::Starting,
        program_fsm::ProgramState::Running => chay_proto::ProgramState::Running,
        program_fsm::ProgramState::Stopping => chay_proto::ProgramState::Stopping,
        program_fsm::ProgramState::Exiting => chay_proto::ProgramState::Exiting,
    }
}

pub fn proto_program_event_result_from_machine_result(
    machine_result: &chay::fsm::MachineResult,
) -> chay_proto::ProgramEventResult {
    match machine_result {
        Ok(Some(message)) => chay_proto::ProgramEventResult {
            result: Some(chay_proto::program_event_result::Result::Ok(
                chay_proto::program_event_result::Ok {
                    message: message.clone(),
                },
            )),
        },
        Ok(None) => chay_proto::ProgramEventResult {
            result: Some(chay_proto::program_event_result::Result::Ok(
                chay_proto::program_event_result::Ok::default(),
            )),
        },
        Err(message) => chay_proto::ProgramEventResult {
            result: Some(chay_proto::program_event_result::Result::Err(
                chay_proto::program_event_result::Err {
                    message: message.clone(),
                },
            )),
        },
    }
}

pub fn proto_start_response_from_program_events_results(
    program_events_results: &HashMap<String, chay::fsm::MachineResult>,
) -> ChaydServiceStartResponse {
    let mut response = ChaydServiceStartResponse::default();
    program_events_results
        .iter()
        .for_each(|(program_name, machine_result)| {
            response.program_event_results.insert(
                program_name.clone(),
                proto_program_event_result_from_machine_result(&machine_result),
            );
        });
    response
}

pub fn proto_stop_response_from_program_events_results(
    program_events_results: &HashMap<String, chay::fsm::MachineResult>,
) -> ChaydServiceStopResponse {
    let mut response = ChaydServiceStopResponse::default();
    program_events_results
        .iter()
        .for_each(|(program_name, machine_result)| {
            response.program_event_results.insert(
                program_name.clone(),
                proto_program_event_result_from_machine_result(&machine_result),
            );
        });
    response
}

pub fn proto_restart_response_from_program_events_results(
    program_events_results: &HashMap<String, chay::fsm::MachineResult>,
) -> ChaydServiceRestartResponse {
    let mut response = ChaydServiceRestartResponse::default();
    program_events_results
        .iter()
        .for_each(|(program_name, machine_result)| {
            response.program_event_results.insert(
                program_name.clone(),
                proto_program_event_result_from_machine_result(&machine_result),
            );
        });
    response
}
