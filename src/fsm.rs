use std::collections::HashMap;

pub trait State<StateKey, AppContext, Event> {
    fn update(&mut self, _context: &mut dyn Context<StateKey>, _app_context: &mut AppContext) {}

    fn react(
        &mut self,
        _event: &Event,
        _context: &mut dyn Context<StateKey>,
        _app_context: &mut AppContext,
    ) -> MachineResult {
        Ok(None)
    }

    fn enter(&mut self, _app_context: &mut AppContext) {}
    fn exit(&mut self, _app_context: &mut AppContext) {}
}

pub trait Context<StateKey> {
    fn transition(&mut self, state_key: StateKey);
}

struct ContextImpl<StateKey> {
    current_state_key: StateKey,
}

impl<StateKey> ContextImpl<StateKey> {
    fn new(init_state_key: StateKey) -> Self {
        ContextImpl::<StateKey> {
            current_state_key: init_state_key,
        }
    }
}

impl<StateKey> Context<StateKey> for ContextImpl<StateKey> {
    fn transition(&mut self, state_key: StateKey) {
        self.current_state_key = state_key;
    }
}

pub type MachineResult = std::result::Result<Option<String>, String>;

pub struct Machine<StateKey, AppContext, Event> {
    app_context: AppContext,
    states: HashMap<StateKey, Box<dyn State<StateKey, AppContext, Event>>>,
    context: ContextImpl<StateKey>,
    first_update: bool,
}

impl<StateKey, AppContext, Event> Machine<StateKey, AppContext, Event>
where
    StateKey: Clone + Eq + std::hash::Hash,
{
    pub fn new(
        app_context: AppContext,
        init_state: StateKey,
        states: HashMap<StateKey, Box<dyn State<StateKey, AppContext, Event>>>,
    ) -> Self {
        return Machine::<StateKey, AppContext, Event> {
            app_context,
            states,
            context: ContextImpl::<StateKey>::new(init_state),
            first_update: true,
        };
    }

    pub fn current_state_key(&self) -> StateKey {
        return self.context.current_state_key.clone();
    }

    pub fn app_context(&self) -> &AppContext {
        &self.app_context
    }

    pub fn update(&mut self) {
        self.maybe_enter_on_first_update();
        let state_key = self.current_state_key();
        let state = self.states.get_mut(&state_key).unwrap();
        state.update(&mut self.context, &mut self.app_context);
        self.maybe_change_state(state_key);
    }

    pub fn react(&mut self, event: &Event) -> MachineResult {
        self.maybe_enter_on_first_update();
        let state_key = self.current_state_key();
        let state = self.states.get_mut(&state_key).unwrap();
        let result = state.react(event, &mut self.context, &mut self.app_context);
        self.maybe_change_state(state_key);
        result
    }

    fn maybe_enter_on_first_update(&mut self) {
        let state_key = self.current_state_key();
        {
            let state = self.states.get_mut(&state_key).unwrap();
            if self.first_update {
                self.first_update = false;
                // Ensure we call the enter method for the initial state before doing anything.
                state.enter(&mut self.app_context);
            }
        }
    }

    fn maybe_change_state(&mut self, old_state_key: StateKey) {
        let new_state_key = self.current_state_key();
        if new_state_key != old_state_key {
            {
                let old_state = self.states.get_mut(&old_state_key).unwrap();
                old_state.exit(&mut self.app_context);
            }
            let new_state = self.states.get_mut(&new_state_key).unwrap();
            new_state.enter(&mut self.app_context);
        }
    }
}
