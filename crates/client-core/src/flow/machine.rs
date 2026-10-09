//! The part that every flow shares: one state, at most one running action, the last error.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::{FlowError, FlowView};

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) struct Machine<S: 'static, K: 'static> {
    inner: Mutex<Inner<S, K>>,
    actions: fn(&S) -> &'static [K],
}

struct Inner<S: 'static, K: 'static> {
    state: S,
    in_progress: Option<K>,
    error: Option<FlowError>,
}

impl<S: Clone, K: Copy + PartialEq + 'static> Machine<S, K> {
    pub(crate) fn new(state: S, actions: fn(&S) -> &'static [K]) -> Self {
        Self {
            inner: Mutex::new(Inner {
                state,
                in_progress: None,
                error: None,
            }),
            actions,
        }
    }

    pub(crate) fn view(&self) -> FlowView<S, K> {
        let inner = lock(&self.inner);
        let actions = match inner.in_progress {
            Some(_) => Vec::new(),
            None => (self.actions)(&inner.state).to_vec(),
        };
        FlowView {
            state: inner.state.clone(),
            in_progress: inner.in_progress,
            actions,
            error: inner.error.clone(),
        }
    }

    /// Changes the state while an action runs, so that an action over several servers
    /// shows the result of each server when it arrives and not when the last one ends.
    pub(crate) fn update(&self, change: impl FnOnce(&mut S)) {
        change(&mut lock(&self.inner).state);
    }

    pub(crate) fn state(&self) -> S {
        lock(&self.inner).state.clone()
    }

    /// Starts an action and returns the state that it starts from. Refuses while another
    /// action runs and when the state does not accept the action; neither refusal changes
    /// the flow.
    pub(crate) fn begin(self: &Arc<Self>, kind: K) -> Result<(S, Run<S, K>), FlowError> {
        let mut inner = lock(&self.inner);
        if inner.in_progress.is_some() {
            return Err(FlowError::Busy);
        }
        if !(self.actions)(&inner.state).contains(&kind) {
            return Err(FlowError::NotAvailable);
        }
        inner.in_progress = Some(kind);
        inner.error = None;
        let run = Run {
            machine: self.clone(),
            finished: false,
        };
        Ok((inner.state.clone(), run))
    }
}

/// A running action. It ends with [`Run::finish`]. When it is dropped without that, the
/// flow returns to its state with the error [`FlowError::Stopped`], so no flow stays busy.
pub(crate) struct Run<S: 'static, K: 'static> {
    machine: Arc<Machine<S, K>>,
    finished: bool,
}

impl<S: 'static, K: 'static> Run<S, K> {
    pub(crate) fn finish(mut self, outcome: Outcome<S>) -> Result<(), FlowError> {
        self.finished = true;
        let mut inner = lock(&self.machine.inner);
        inner.in_progress = None;
        if let Some(state) = outcome.next {
            inner.state = state;
        }
        inner.error = outcome.error.clone();
        match outcome.error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl<S: 'static, K: 'static> Drop for Run<S, K> {
    fn drop(&mut self) {
        if !self.finished {
            let mut inner = lock(&self.machine.inner);
            inner.in_progress = None;
            inner.error = Some(FlowError::Stopped);
        }
    }
}

/// How an action ended: the next state when the flow moves, and the reason when the action
/// failed. A failure without a next state keeps the state.
pub(crate) struct Outcome<S> {
    next: Option<S>,
    error: Option<FlowError>,
}

impl<S> Outcome<S> {
    pub(crate) fn moved(next: S) -> Self {
        Self {
            next: Some(next),
            error: None,
        }
    }

    pub(crate) fn failed(error: impl Into<FlowError>) -> Self {
        Self {
            next: None,
            error: Some(error.into()),
        }
    }

    pub(crate) fn failed_and_moved(next: S, error: impl Into<FlowError>) -> Self {
        Self {
            next: Some(next),
            error: Some(error.into()),
        }
    }
}

impl<S> From<Result<S, FlowError>> for Outcome<S> {
    fn from(result: Result<S, FlowError>) -> Self {
        match result {
            Ok(next) => Self::moved(next),
            Err(error) => Self::failed(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Light {
        Red,
        Green,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Press {
        Go,
        Stop,
    }

    fn accepted(state: &Light) -> &'static [Press] {
        match state {
            Light::Red => &[Press::Go],
            Light::Green => &[Press::Stop],
        }
    }

    #[test]
    fn an_action_runs_alone_and_only_from_a_state_that_accepts_it() {
        let machine = Arc::new(Machine::new(Light::Red, accepted));
        assert_eq!(machine.view().actions, [Press::Go]);
        assert_eq!(
            machine.begin(Press::Stop).err(),
            Some(FlowError::NotAvailable)
        );
        assert_eq!(machine.view().error, None);

        let (before, run) = machine.begin(Press::Go).unwrap();
        assert_eq!(before, Light::Red);
        let running = machine.view();
        assert_eq!(running.state, Light::Red);
        assert_eq!(running.in_progress, Some(Press::Go));
        assert!(running.actions.is_empty());
        assert_eq!(machine.begin(Press::Go).err(), Some(FlowError::Busy));
        assert_eq!(machine.view().in_progress, Some(Press::Go));

        run.finish(Outcome::moved(Light::Green)).unwrap();
        let done = machine.view();
        assert_eq!(done.state, Light::Green);
        assert_eq!(done.in_progress, None);
        assert_eq!(done.actions, [Press::Stop]);
    }

    #[test]
    fn a_failure_keeps_the_state_and_the_next_action_clears_the_reason() {
        let machine = Arc::new(Machine::new(Light::Red, accepted));
        let (_, run) = machine.begin(Press::Go).unwrap();
        assert_eq!(
            run.finish(Outcome::failed(FlowError::Unavailable)),
            Err(FlowError::Unavailable)
        );
        let failed = machine.view();
        assert_eq!(failed.state, Light::Red);
        assert_eq!(failed.error, Some(FlowError::Unavailable));
        assert_eq!(failed.actions, [Press::Go]);

        let (_, run) = machine.begin(Press::Go).unwrap();
        assert_eq!(machine.view().error, None);
        let moved = run.finish(Outcome::failed_and_moved(
            Light::Green,
            FlowError::InvalidResponse,
        ));
        assert_eq!(moved, Err(FlowError::InvalidResponse));
        assert_eq!(machine.view().state, Light::Green);
    }

    #[test]
    fn an_action_that_is_dropped_before_its_end_leaves_the_flow_usable() {
        let machine = Arc::new(Machine::new(Light::Red, accepted));
        let (_, run) = machine.begin(Press::Go).unwrap();
        drop(run);
        let view = machine.view();
        assert_eq!(view.state, Light::Red);
        assert_eq!(view.in_progress, None);
        assert_eq!(view.error, Some(FlowError::Stopped));
        assert!(machine.begin(Press::Go).is_ok());
    }
}
