//! Pure parent-owned engine lifecycle. Effects are requests, never OS facts.
//!
//! The adapter must hold one coordinator lock while applying each event, then
//! execute effects outside that lock. `LeaseAcquired`, `TreeExited` and
//! `LeaseReleased` require actual native evidence; a timer/EOF/launcher exit is
//! not that evidence. No path, cookie, OS handle or wall clock lives here.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    pub generation: u64,
    pub attempt: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Sleeping,
    Starting,
    Running,
    Preparing,
    Stopping,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    Wake,
    Edit,
    Close,
    Lock,
    Shutdown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    Ineligible,
    InvalidSnapshot,
    PrepareRefused,
    PrepareTimeout,
    StopTimeout,
    StartFailure,
    UnexpectedExit,
    CounterExhausted,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopKind {
    Hibernate,
    Edit,
    Close,
    Shutdown,
    StartFailure,
    Crash,
}

/// All tokens come from effects/current stamp, never from a website. Eligibility
/// revisions must change on selection/views/activity/routes/lock changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    StartRequested {
        generation: u64,
    },
    LeaseAcquired {
        stamp: Stamp,
    },
    LeaseFailed {
        stamp: Stamp,
    },
    StartReady {
        stamp: Stamp,
    },
    StartFailed {
        stamp: Stamp,
    },
    PrepareRequested {
        generation: u64,
        eligibility_revision: u64,
        eligible: bool,
    },
    Prepared {
        stamp: Stamp,
        eligibility_revision: u64,
        snapshot_valid: bool,
    },
    Commit {
        stamp: Stamp,
        eligibility_revision: u64,
    },
    Refused {
        stamp: Stamp,
        eligibility_revision: u64,
    },
    AbortAck {
        stamp: Stamp,
        eligibility_revision: u64,
    },
    EligibilityChanged {
        revision: u64,
    },
    ShutdownAck {
        stamp: Stamp,
    },
    /// Proven exit of the ENTIRE owned tree, not just its launcher.
    TreeExited {
        generation: u64,
    },
    LeaseReleased {
        stamp: Stamp,
    },
    Deadline {
        stamp: Stamp,
    },
    IntentRequested(Intent),
    Unlocked,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    AcquireLease {
        stamp: Stamp,
    },
    Spawn {
        stamp: Stamp,
    },
    Prepare {
        stamp: Stamp,
        eligibility_revision: u64,
    },
    AbortPrepare {
        stamp: Stamp,
        eligibility_revision: u64,
    },
    Stop {
        stamp: Stamp,
        kind: StopKind,
    },
    ReleaseRoutes {
        stamp: Stamp,
    },
    ReleaseLease {
        stamp: Stamp,
    },
    PublishSleeping {
        stamp: Stamp,
    },
    ApplyEdit,
    Closed,
    ShutdownComplete,
    Report {
        stamp: Stamp,
        reason: Reason,
    },
}

#[derive(Debug, Serialize)]
pub struct Machine {
    phase: Phase,
    stamp: Stamp,
    profile_reserved: bool,
    lease_confirmed: bool,
    locked: bool,
    pending_intent: Option<Intent>,
    wake_pending: bool,
    eligibility_revision: u64,
    snapshot_ready: bool,
    abort_pending: bool,
    stop_kind: Option<StopKind>,
    shutdown_ack: bool,
    tree_exited: bool,
    releasing: bool,
    successful_hibernation: bool,
    terminal: bool,
}
impl Default for Machine {
    fn default() -> Self {
        Self::new()
    }
}
impl Machine {
    /// Initial Sleeping only means no engine was started, NOT hibernation success.
    pub fn new() -> Self {
        Self {
            phase: Phase::Sleeping,
            stamp: Stamp::default(),
            profile_reserved: false,
            lease_confirmed: false,
            locked: false,
            pending_intent: None,
            wake_pending: false,
            eligibility_revision: 0,
            snapshot_ready: false,
            abort_pending: false,
            stop_kind: None,
            shutdown_ack: false,
            tree_exited: false,
            releasing: false,
            successful_hibernation: false,
            terminal: false,
        }
    }
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn stamp(&self) -> Stamp {
        self.stamp
    }
    pub fn profile_reserved(&self) -> bool {
        self.profile_reserved
    }
    pub fn lease_confirmed(&self) -> bool {
        self.lease_confirmed
    }
    pub fn successful_hibernation(&self) -> bool {
        self.successful_hibernation
    }
    pub fn pending_intent(&self) -> Option<Intent> {
        self.pending_intent
    }
    pub fn locked(&self) -> bool {
        self.locked
    }
    pub fn terminal(&self) -> bool {
        self.terminal
    }
    fn report(&self, out: &mut Vec<Effect>, reason: Reason) {
        out.push(Effect::Report {
            stamp: self.stamp,
            reason,
        });
    }
    fn next_attempt(&mut self, out: &mut Vec<Effect>) -> bool {
        if let Some(n) = self.stamp.attempt.checked_add(1) {
            self.stamp.attempt = n;
            true
        } else {
            self.report(out, Reason::CounterExhausted);
            false
        }
    }
    fn start(&mut self, out: &mut Vec<Effect>) {
        if self.phase != Phase::Sleeping || self.profile_reserved || self.locked || self.terminal {
            return;
        }
        let Some(generation) = self.stamp.generation.checked_add(1) else {
            self.report(out, Reason::CounterExhausted);
            return;
        };
        self.stamp = Stamp {
            generation,
            attempt: 0,
        };
        self.phase = Phase::Starting;
        self.profile_reserved = true; // Logical reservation, not OS lease.
        self.lease_confirmed = false;
        self.successful_hibernation = false;
        self.wake_pending = false;
        self.snapshot_ready = false;
        self.tree_exited = false;
        self.shutdown_ack = false;
        self.releasing = false;
        self.stop_kind = None;
        out.push(Effect::AcquireLease { stamp: self.stamp });
    }
    fn abort(&mut self, out: &mut Vec<Effect>) {
        if self.phase == Phase::Preparing && !self.abort_pending {
            self.abort_pending = true;
            out.push(Effect::AbortPrepare {
                stamp: self.stamp,
                eligibility_revision: self.eligibility_revision,
            });
        }
    }
    fn begin_stop(&mut self, kind: StopKind, out: &mut Vec<Effect>) {
        if !self.next_attempt(out) {
            return;
        }
        self.phase = Phase::Stopping;
        self.stop_kind = Some(kind);
        self.shutdown_ack = false;
        self.tree_exited = false;
        self.releasing = false;
        self.snapshot_ready = false;
        out.push(Effect::Stop {
            stamp: self.stamp,
            kind,
        });
    }
    fn queue(&mut self, intent: Intent) {
        // Shutdown > Close > Edit > Wake. Lock is orthogonal, never erases close.
        if self.stop_kind == Some(StopKind::Shutdown) {
            return;
        }
        if self.stop_kind == Some(StopKind::Close) && intent != Intent::Shutdown {
            return;
        }
        match intent {
            Intent::Shutdown => {
                self.pending_intent = Some(intent);
                self.wake_pending = false;
            }
            Intent::Close if self.pending_intent != Some(Intent::Shutdown) => {
                self.pending_intent = Some(intent);
                self.wake_pending = false;
            }
            Intent::Edit
                if !matches!(self.pending_intent, Some(Intent::Close | Intent::Shutdown)) =>
            {
                self.pending_intent = Some(intent);
                self.wake_pending = true;
            }
            Intent::Wake
                if !matches!(self.pending_intent, Some(Intent::Close | Intent::Shutdown)) =>
            {
                self.wake_pending = true;
            }
            _ => {}
        }
    }
    fn drain_running(&mut self, out: &mut Vec<Effect>) {
        self.abort_pending = false;
        self.snapshot_ready = false;
        match self.pending_intent.take() {
            Some(Intent::Shutdown) => self.begin_stop(StopKind::Shutdown, out),
            Some(Intent::Close) => self.begin_stop(StopKind::Close, out),
            Some(Intent::Edit) => self.begin_stop(StopKind::Edit, out),
            _ => {
                self.wake_pending = false;
            }
        }
    }
    fn finish_stop(&mut self, out: &mut Vec<Effect>) {
        if self.phase != Phase::Stopping
            || self.releasing
            || !self.tree_exited
            || !self.shutdown_ack
        {
            return;
        }
        if self.stop_kind == Some(StopKind::Hibernate) && !self.snapshot_ready {
            return;
        }
        self.releasing = true;
        // Ordered adapter effects: stop old network resources, then release OS
        // lease, then deliver LeaseReleased. No replacement before that event.
        out.push(Effect::ReleaseRoutes { stamp: self.stamp });
        out.push(Effect::ReleaseLease { stamp: self.stamp });
    }
    pub fn apply(&mut self, event: Event) -> Vec<Effect> {
        let mut out = Vec::new();
        match event {
            Event::StartRequested { generation } if generation == self.stamp.generation => {
                self.start(&mut out)
            }
            Event::LeaseAcquired { stamp }
                if stamp == self.stamp
                    && self.phase == Phase::Starting
                    && !self.lease_confirmed =>
            {
                self.lease_confirmed = true;
                if self.locked || self.pending_intent.is_some() {
                    // No child was spawned; adapter still confirms empty tree,
                    // shutdown and release. Never silently drop an acquired lease.
                    self.begin_stop(
                        match self.pending_intent {
                            Some(Intent::Close) => StopKind::Close,
                            Some(Intent::Shutdown) => StopKind::Shutdown,
                            Some(Intent::Edit) => StopKind::Edit,
                            _ => StopKind::StartFailure,
                        },
                        &mut out,
                    );
                } else {
                    out.push(Effect::Spawn { stamp });
                }
            }
            Event::LeaseFailed { stamp }
                if stamp == self.stamp
                    && self.phase == Phase::Starting
                    && !self.lease_confirmed =>
            {
                self.phase = Phase::Sleeping;
                self.profile_reserved = false;
                self.report(&mut out, Reason::StartFailure);
                self.complete_idle(&mut out, false);
            }
            Event::StartReady { stamp }
                if stamp == self.stamp && self.phase == Phase::Starting && self.lease_confirmed =>
            {
                self.phase = Phase::Running;
                self.drain_running(&mut out);
            }
            Event::StartFailed { stamp }
                if stamp == self.stamp && self.phase == Phase::Starting && self.lease_confirmed =>
            {
                self.phase = Phase::Stopping;
                self.stop_kind = Some(StopKind::StartFailure);
                self.shutdown_ack = true;
                self.report(&mut out, Reason::StartFailure);
                out.push(Effect::Stop {
                    stamp,
                    kind: StopKind::StartFailure,
                });
                // OS cleanup must finish before TreeExited/lease release.
            }
            Event::PrepareRequested {
                generation,
                eligibility_revision,
                eligible,
            } if generation == self.stamp.generation && self.phase == Phase::Running => {
                if !eligible || self.locked {
                    self.report(&mut out, Reason::Ineligible);
                } else if self.next_attempt(&mut out) {
                    self.phase = Phase::Preparing;
                    self.eligibility_revision = eligibility_revision;
                    self.abort_pending = false;
                    self.snapshot_ready = false;
                    out.push(Effect::Prepare {
                        stamp: self.stamp,
                        eligibility_revision,
                    });
                }
            }
            Event::Prepared {
                stamp,
                eligibility_revision,
                snapshot_valid,
            } if self.preparing(stamp, eligibility_revision) && !self.abort_pending => {
                if snapshot_valid {
                    self.snapshot_ready = true;
                } else {
                    self.report(&mut out, Reason::InvalidSnapshot);
                    self.abort(&mut out);
                }
            }
            Event::Commit {
                stamp,
                eligibility_revision,
            } if self.preparing(stamp, eligibility_revision)
                && !self.abort_pending
                && self.snapshot_ready
                && !self.locked =>
            {
                self.phase = Phase::Stopping;
                self.stop_kind = Some(StopKind::Hibernate);
                self.shutdown_ack = false;
                self.tree_exited = false;
                self.releasing = false;
                out.push(Effect::Stop {
                    stamp,
                    kind: StopKind::Hibernate,
                });
            }
            Event::Refused {
                stamp,
                eligibility_revision,
            } if self.preparing(stamp, eligibility_revision) => {
                self.report(&mut out, Reason::PrepareRefused);
                self.phase = Phase::Running;
                self.drain_running(&mut out);
            }
            Event::AbortAck {
                stamp,
                eligibility_revision,
            } if self.preparing(stamp, eligibility_revision) && self.abort_pending => {
                self.phase = Phase::Running;
                self.drain_running(&mut out);
            }
            Event::EligibilityChanged { revision }
                if self.phase == Phase::Preparing && revision != self.eligibility_revision =>
            {
                self.abort(&mut out)
            }
            Event::ShutdownAck { stamp }
                if stamp == self.stamp && self.phase == Phase::Stopping =>
            {
                self.shutdown_ack = true;
                self.finish_stop(&mut out);
            }
            Event::TreeExited { generation }
                if generation == self.stamp.generation
                    && self.profile_reserved
                    && self.lease_confirmed
                    && !self.releasing =>
            {
                if self.phase != Phase::Stopping {
                    self.phase = Phase::Stopping;
                    self.stop_kind = Some(StopKind::Crash);
                    self.shutdown_ack = true;
                    self.snapshot_ready = false;
                    self.report(&mut out, Reason::UnexpectedExit);
                }
                self.tree_exited = true;
                self.finish_stop(&mut out);
            }
            Event::LeaseReleased { stamp }
                if stamp == self.stamp && self.phase == Phase::Stopping && self.releasing =>
            {
                self.profile_reserved = false;
                self.lease_confirmed = false;
                self.phase = Phase::Sleeping;
                self.successful_hibernation = self.stop_kind == Some(StopKind::Hibernate)
                    && self.snapshot_ready
                    && self.shutdown_ack
                    && self.tree_exited
                    && !matches!(
                        self.pending_intent,
                        Some(Intent::Edit | Intent::Close | Intent::Shutdown)
                    );
                if self.successful_hibernation {
                    out.push(Effect::PublishSleeping { stamp });
                }
                self.complete_idle(&mut out, true);
            }
            Event::Deadline { stamp } if stamp == self.stamp => match self.phase {
                Phase::Preparing => {
                    self.report(&mut out, Reason::PrepareTimeout);
                    self.abort(&mut out);
                }
                Phase::Stopping => self.report(&mut out, Reason::StopTimeout),
                _ => {}
            },
            Event::IntentRequested(Intent::Lock) => {
                self.locked = true;
                if self.phase == Phase::Starting {
                    self.wake_pending = true;
                }
                self.abort(&mut out);
            }
            Event::IntentRequested(intent) => {
                self.queue(intent);
                match self.phase {
                    Phase::Sleeping => self.complete_idle(&mut out, true),
                    Phase::Running => {
                        if intent != Intent::Wake {
                            self.drain_running(&mut out);
                        }
                    }
                    Phase::Preparing => self.abort(&mut out),
                    Phase::Starting | Phase::Stopping => {}
                }
            }
            Event::Unlocked => {
                self.locked = false;
                if self.phase == Phase::Sleeping {
                    self.complete_idle(&mut out, true);
                }
            }
            _ => {} // stale/duplicate/out-of-phase native completions are inert.
        }
        debug_assert!(self.phase == Phase::Sleeping || self.profile_reserved);
        debug_assert!(!self.lease_confirmed || self.profile_reserved);
        out
    }
    fn preparing(&self, stamp: Stamp, revision: u64) -> bool {
        self.phase == Phase::Preparing
            && stamp == self.stamp
            && revision == self.eligibility_revision
    }
    fn complete_idle(&mut self, out: &mut Vec<Effect>, allow_start: bool) {
        // Explicit intents override the prior stop reason, including during
        // hibernation. They never manufacture a successful-hibernation receipt.
        let intent = self.pending_intent.take().or(match self.stop_kind {
            Some(StopKind::Close) => Some(Intent::Close),
            Some(StopKind::Shutdown) => Some(Intent::Shutdown),
            Some(StopKind::Edit) => Some(Intent::Edit),
            _ => None,
        });
        match intent {
            Some(Intent::Shutdown) => {
                self.terminal = true;
                self.wake_pending = false;
                out.push(Effect::ShutdownComplete);
            }
            Some(Intent::Close) => {
                self.terminal = true;
                self.wake_pending = false;
                out.push(Effect::Closed);
            }
            Some(Intent::Edit) => {
                out.push(Effect::ApplyEdit);
                self.wake_pending = true;
            }
            _ => {}
        }
        self.stop_kind = None;
        if allow_start && self.wake_pending && !self.terminal {
            self.start(out);
        }
    }
}
