//! Short synchronous lock-intent linearization; no vault, IO, async lock or UI.
//! Register before dispatching lock work. Capture before waiting on unlock's
//! serial mutex. Only try_unlock(ticket) may commit an authorized unlock.
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Mutex,
};
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Ticket(u64);
pub struct IntentGate {
    state: AtomicU64,
    intent: AtomicU64,
    enabled: AtomicBool,
    exhausted: AtomicBool,
    decision: Mutex<()>,
}
impl IntentGate {
    pub const fn new() -> Self {
        Self {
            state: AtomicU64::new(1),
            intent: AtomicU64::new(0),
            enabled: AtomicBool::new(false),
            exhausted: AtomicBool::new(false),
            decision: Mutex::new(()),
        }
    }
    fn change(&self, locked: bool) {
        let _ = self
            .state
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |old| {
                Some(((old & !1).wrapping_add(2)) | u64::from(locked))
            });
    }
    pub fn engine_state(&self) -> (bool, u64) {
        let state = self.state.load(Ordering::SeqCst);
        (state & 1 != 0, state >> 1)
    }
    pub fn configure(&self, exists: bool) {
        let _guard = self.decision.lock().unwrap_or_else(|e| e.into_inner());
        self.enabled.store(exists, Ordering::SeqCst);
        self.change(exists);
    }
    pub fn enable(&self) {
        let _guard = self.decision.lock().unwrap_or_else(|e| e.into_inner());
        self.enabled.store(true, Ordering::SeqCst);
    }
    pub fn transition(&self, locked: bool) {
        let _guard = self.decision.lock().unwrap_or_else(|e| e.into_inner());
        self.change(locked);
    }
    pub fn ticket(&self) -> Ticket {
        Ticket(self.intent.load(Ordering::SeqCst))
    }
    pub fn is_current(&self, ticket: Ticket) -> bool {
        !self.exhausted.load(Ordering::SeqCst) && self.intent.load(Ordering::SeqCst) == ticket.0
    }
    pub fn register_lock(&self) {
        let _guard = self.decision.lock().unwrap_or_else(|e| e.into_inner());
        if self
            .intent
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |old| old.checked_add(1))
            .is_err()
        {
            self.exhausted.store(true, Ordering::SeqCst);
        }
        if self.enabled.load(Ordering::SeqCst) {
            self.change(true);
        }
    }
    /// Only the atomic authorization/state decision is serialized. Never hold this
    /// mutex across native window calls; UI dispatch can synchronously reenter.
    pub fn try_unlock(&self, ticket: Ticket) -> bool {
        let _guard = self.decision.lock().unwrap_or_else(|e| e.into_inner());
        if !self.is_current(ticket) {
            return false;
        }
        self.change(false);
        true
    }
}
impl Default for IntentGate {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Barrier;
    #[test]
    fn no_vault_shortcut_does_not_create_an_impossible_password_gate() {
        let gate = IntentGate::new();
        gate.configure(false);
        let ticket = gate.ticket();
        gate.register_lock();
        assert!(!gate.engine_state().0);
        assert!(!gate.is_current(ticket));
    }
    #[test]
    fn configured_fresh_intent_locks_synchronously_and_advances_generation() {
        let gate = IntentGate::new();
        gate.configure(true);
        let ticket = gate.ticket();
        assert!(gate.try_unlock(ticket));
        let previous = gate.engine_state().1;
        gate.register_lock();
        assert!(gate.engine_state().0);
        assert!(gate.engine_state().1 > previous);
        assert!(!gate.try_unlock(ticket));
    }
    #[tokio::test]
    async fn fresh_press_during_serial_wait_invalidates_real_unlock_ticket() {
        let gate = Arc::new(IntentGate::new());
        gate.configure(true);
        let entered = Arc::new(Barrier::new(2));
        let serial = Arc::new(tokio::sync::Mutex::new(()));
        let serial_guard = serial.lock().await;
        let worker = {
            let gate = gate.clone();
            let entered = entered.clone();
            let serial = serial.clone();
            tokio::spawn(async move {
                let ticket = gate.ticket();
                entered.wait().await;
                let _guard = serial.lock().await;
                gate.try_unlock(ticket)
            })
        };
        entered.wait().await;
        gate.register_lock();
        drop(serial_guard);
        assert!(!worker.await.unwrap());
        assert!(gate.engine_state().0);
    }
    #[tokio::test]
    async fn fresh_press_after_work_before_commit_invalidates_the_ticket() {
        let gate = Arc::new(IntentGate::new());
        gate.configure(true);
        let mutated = Arc::new(Barrier::new(2));
        let resume = Arc::new(Barrier::new(2));
        let worker = {
            let gate = gate.clone();
            let mutated = mutated.clone();
            let resume = resume.clone();
            tokio::spawn(async move {
                let ticket = gate.ticket();
                mutated.wait().await;
                resume.wait().await;
                (gate.try_unlock(ticket), gate.engine_state().0)
            })
        };
        mutated.wait().await;
        gate.register_lock();
        resume.wait().await;
        assert_eq!(worker.await.unwrap(), (false, true));
    }
    #[test]
    fn intent_after_a_committed_unlock_is_a_new_pessimistic_lock() {
        let gate = IntentGate::new();
        gate.configure(true);
        assert!(gate.try_unlock(gate.ticket()));
        gate.register_lock();
        assert!(gate.engine_state().0);
        assert!(gate.try_unlock(gate.ticket()));
        assert!(!gate.engine_state().0);
    }
    #[test]
    fn counter_exhaustion_can_never_revalidate_an_old_ticket() {
        let gate = IntentGate::new();
        gate.configure(true);
        gate.intent.store(u64::MAX, Ordering::SeqCst);
        let ticket = gate.ticket();
        gate.register_lock();
        assert!(!gate.is_current(ticket));
        assert!(!gate.try_unlock(ticket));
        assert!(gate.engine_state().0);
    }
}
