//! Parent-only automatic Super sleep. Timer/EOF are never exit evidence.
use crate::{
    sessiya,
    super_cookie_escrow::{self, CookieSnapshot, EphemeralKey, Scope},
    super_hibernation_protocol::{Frame, Prepare, Snapshot, Token, WireKey},
    super_lifecycle::{Effect, Event, Intent, Machine, Phase, Stamp, StopKind},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
use tauri::{Emitter, Manager};
use zeroize::Zeroizing;
const GENERATION_RANGE: u64 = 1 << 32;
static NEXT_RANGE: AtomicU64 = AtomicU64::new(1);
const TOTAL_ESCROW: usize = 64 * 1024 * 1024;

pub(crate) struct Node {
    pub machine: Machine,
    last_active: Instant,
    revision: u64,
    operation_revision: Option<u64>,
    end_generation: u64,
    pub deadline: Option<Instant>,
    pub snapshot: Option<Snapshot>,
    key: Option<EphemeralKey>,
    scope: Option<Scope>,
    pub reason: Option<&'static str>,
    pub release_pending: bool,
    final_received: bool,
    restored_received: bool,
    eligible: bool,
    pending_control: Option<(sessiya::Komanda, u8)>,
}
impl Default for Node {
    fn default() -> Self {
        let seed = NEXT_RANGE
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                n.checked_add(GENERATION_RANGE)
            })
            .unwrap_or(u64::MAX);
        Self {
            machine: Machine::new_at_generation(seed.saturating_sub(1)),
            last_active: Instant::now(),
            revision: 1,
            operation_revision: None,
            end_generation: seed
                .saturating_add(GENERATION_RANGE - 1)
                .min((1_u64 << 53) - 1),
            deadline: None,
            snapshot: None,
            key: None,
            scope: None,
            reason: Some("cold"),
            release_pending: false,
            final_received: false,
            restored_received: false,
            eligible: false,
            pending_control: None,
        }
    }
}
fn token(stamp: Stamp, revision: u64) -> Token {
    Token {
        generation: stamp.generation,
        attempt: stamp.attempt,
        revision,
    }
}
fn stamp(token: &Token) -> Stamp {
    Stamp {
        generation: token.generation,
        attempt: token.attempt,
    }
}
fn channel_send(
    channel: Option<&std::sync::mpsc::SyncSender<sessiya::Komanda>>,
    command: sessiya::Komanda,
) -> bool {
    channel.is_some_and(|c| c.try_send(command).is_ok())
}
impl Node {
    pub fn begin_start(&mut self) -> Result<u64, &'static str> {
        // Unlock IPC may legitimately start a cold Super before the next
        // low-frequency policy tick. Synchronize that actual global state now.
        if self.machine.locked() && !crate::browser_lock::locked_global() {
            let effects = self.machine.apply(Event::Unlocked);
            self.effects(effects, None);
        }
        if self.machine.phase() == Phase::Sleeping {
            if self.machine.stamp().generation >= self.end_generation {
                return Err("generation_exhausted");
            }
            let generation = self.machine.stamp().generation;
            self.machine.apply(Event::StartRequested { generation });
        }
        if self.machine.phase() != Phase::Starting {
            return Err("previous_owner_not_released");
        }
        self.reason = Some("starting");
        self.last_active = Instant::now();
        Ok(self.machine.stamp().generation)
    }
    pub fn lease_acquired(&mut self, generation: u64) -> bool {
        self.machine
            .apply(Event::LeaseAcquired {
                stamp: Stamp {
                    generation,
                    attempt: 0,
                },
            })
            .iter()
            .any(|e| matches!(e, Effect::Spawn { .. }))
    }
    pub fn lease_failed(&mut self, generation: u64) {
        self.machine.apply(Event::LeaseFailed {
            stamp: Stamp {
                generation,
                attempt: 0,
            },
        });
        self.reason = Some("profile_busy");
    }
    /// Explicit/crash teardown is not hibernation success. The worker retains
    /// the actual guard; observations report unknown membership until exit.
    pub fn explicit_stop(&mut self) {
        if self.machine.phase() == Phase::Starting {
            let stamp = self.machine.stamp();
            self.machine.apply(Event::StartFailed { stamp });
        } else {
            self.machine.apply(Event::IntentRequested(Intent::Close));
        }
        self.reason = Some("explicit_stop");
    }
    pub fn no_child_start_failed(&mut self, generation: u64) {
        let stamp = Stamp {
            generation,
            attempt: 0,
        };
        self.machine.apply(Event::StartFailed { stamp });
        self.machine.apply(Event::TreeExited { generation });
        self.machine.apply(Event::LeaseReleased { stamp });
        self.deadline = None;
        self.release_pending = false;
        self.reason = Some("start_failed");
    }
    pub fn ready(&mut self, generation: u64) {
        if self.snapshot.is_none() || self.restored_received {
            self.clear_escrow();
            self.machine.apply(Event::StartReady {
                stamp: Stamp {
                    generation,
                    attempt: 0,
                },
            });
            self.reason = None;
        }
    }
    pub fn restore_wire(&self) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        let Some(snapshot) = &self.snapshot else {
            return Ok(None);
        };
        let key = self
            .key
            .as_ref()
            .ok_or("Missing transient key")?
            .wire_base64();
        #[derive(Serialize)]
        struct Borrowed<'a> {
            key: &'a str,
            snapshot: &'a Snapshot,
        }
        let mut wire = Zeroizing::new(
            serde_json::to_vec(&Borrowed {
                key: &key,
                snapshot,
            })
            .map_err(|_| "Restore frame refused")?,
        );
        if wire.len() > super_cookie_escrow::MAX_BYTES * 2 {
            return Err("Restore frame too large".into());
        }
        wire.push(b'\n');
        Ok(Some(wire))
    }
    fn control(
        &mut self,
        channel: Option<&std::sync::mpsc::SyncSender<sessiya::Komanda>>,
        command: sessiya::Komanda,
    ) {
        self.pending_control = Some((command, 0));
        self.flush_control(channel);
    }
    fn flush_control(&mut self, channel: Option<&std::sync::mpsc::SyncSender<sessiya::Komanda>>) {
        let Some((command, attempts)) = self.pending_control.take() else {
            return;
        };
        let Some(channel) = channel else {
            self.reason = Some("control_channel_closed");
            return;
        };
        match channel.try_send(command) {
            Ok(()) => {}
            Err(std::sync::mpsc::TrySendError::Full(command)) if attempts < 30 => {
                self.pending_control = Some((command, attempts + 1))
            }
            Err(_) => self.reason = Some("control_delivery_failed"),
        }
    }
    fn effects(
        &mut self,
        effects: Vec<Effect>,
        channel: Option<&std::sync::mpsc::SyncSender<sessiya::Komanda>>,
    ) {
        for effect in effects {
            match effect {
                Effect::AbortPrepare {
                    stamp,
                    eligibility_revision,
                } => {
                    self.deadline = Some(Instant::now() + Duration::from_secs(40));
                    self.control(
                        channel,
                        sessiya::Komanda::HibernateAbort {
                            token: token(stamp, eligibility_revision),
                        },
                    );
                }
                Effect::Stop {
                    stamp,
                    kind: StopKind::Hibernate,
                } => {
                    self.deadline = Some(Instant::now() + Duration::from_secs(60));
                    self.control(
                        channel,
                        sessiya::Komanda::HibernateCommit {
                            token: token(stamp, self.operation_revision.unwrap_or(self.revision)),
                        },
                    );
                }
                Effect::ReleaseRoutes { .. } => self.release_pending = true,
                Effect::Report { reason, .. } => {
                    self.reason = Some(match reason {
                        crate::super_lifecycle::Reason::PrepareTimeout => "prepare_timeout",
                        crate::super_lifecycle::Reason::StopTimeout => "stop_timeout",
                        crate::super_lifecycle::Reason::UnexpectedExit => "unexpected_exit",
                        _ => "refused",
                    })
                }
                _ => {}
            }
        }
    }
    pub fn wake(
        &mut self,
        channel: Option<&std::sync::mpsc::SyncSender<sessiya::Komanda>>,
    ) -> bool {
        self.last_active = Instant::now();
        let waiting = matches!(self.machine.phase(), Phase::Preparing | Phase::Stopping);
        if waiting {
            let effects = self.machine.apply(Event::IntentRequested(Intent::Wake));
            self.effects(effects, channel);
        }
        waiting
    }
    fn invalidate(&mut self, channel: Option<&std::sync::mpsc::SyncSender<sessiya::Komanda>>) {
        self.last_active = Instant::now();
        self.eligible = false;
        self.revision = self.revision.saturating_add(1);
        let effects = self.machine.apply(Event::EligibilityChanged {
            revision: self.revision,
        });
        self.effects(effects, channel);
    }
    fn clear_escrow(&mut self) {
        self.snapshot = None;
        self.key = None;
        self.scope = None;
        self.pending_control = None;
        self.operation_revision = None;
        self.final_received = false;
        self.restored_received = false;
    }
}
fn protected_views(s: &crate::Sostoyanie) -> HashSet<String> {
    let mut ids = HashSet::new();
    if let Ok(windows) = s.okna.0.lock() {
        for window in windows.values() {
            ids.insert(window.super_id.clone());
            ids.extend(window.others.keys().cloned());
            ids.extend(window.pending_close.iter().cloned());
        }
    } else {
        if let Ok(tabs) = s.vkladki.lock() {
            ids.extend(tabs.keys().cloned());
        }
    }
    ids
}
fn idle_period() -> Duration {
    let settings = crate::nastrojki::zagruzit();
    if std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
        && std::env::var_os("NEXUSBRO_TEST_DATA").is_some()
    {
        if let Some(seconds) = std::env::var("NEXUSBRO_TEST_SUPER_IDLE_SECONDS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|v| (2..=1800).contains(v))
        {
            return Duration::from_secs(seconds);
        }
    }
    Duration::from_secs(u64::from(settings.memory_saver_minutes.clamp(1, 1440)) * 60)
}
fn parent_eligible(
    v: &crate::Vkladka,
    id: &str,
    selected: Option<&str>,
    views: &HashSet<String>,
    locked: bool,
) -> bool {
    !locked
        && !v.nastrojki.privatnaya
        && !crate::privatnost::eto_id(id)
        && selected != Some(id)
        && !views.contains(id)
        && !v.zapusk_idet
        && v.proc.is_some()
        && v.kanal.is_some()
        && v.obychnye.as_ref().is_some_and(|session| {
            session
                .vkladki
                .iter()
                .all(|tab| matches!(tab.route, crate::tab_routes::RouteChoice::Inherit))
        })
}
fn snapshot_valid(node: &Node, snapshot: &Snapshot, session: Option<&sessiya::Sessiya>) -> bool {
    let Some(scope) = node.scope.as_ref() else {
        return false;
    };
    if &snapshot.scope != scope
        || stamp(&snapshot.token) != node.machine.stamp()
        || Some(snapshot.token.revision) != node.operation_revision
        || snapshot.scope.context_key.len() != 64
    {
        return false;
    }
    let Some(key) = node.key.as_ref() else {
        return false;
    };
    let Ok(clear) = super_cookie_escrow::open(scope, key, &snapshot.sealed) else {
        return false;
    };
    if CookieSnapshot::from_cdp_result(&clear).is_err() {
        return false;
    }
    let tabs = &snapshot.session.vkladki;
    if tabs.is_empty()
        || tabs.len() > 4096
        || snapshot.scroll.len() > 4096
        || !tabs.iter().any(|t| t.id == snapshot.session.aktivnaya)
    {
        return false;
    }
    let mut ids = HashSet::new();
    if tabs.iter().any(|t| {
        !ids.insert(t.id)
            || t.privatnaya
            || t.route_pending
            || t.memory_pending
            || t.adres.len() > 32768
    }) {
        return false;
    }
    if snapshot.scroll.iter().any(|p| {
        !p.x.is_finite()
            || !p.y.is_finite()
            || !tabs
                .iter()
                .any(|t| t.id == p.id && t.route_generation == p.route_generation)
    }) {
        return false;
    }
    let Some(current) = session else {
        return false;
    };
    if current.aktivnaya != snapshot.session.aktivnaya || current.vkladki.len() != tabs.len() {
        return false;
    }
    current.vkladki.iter().all(|old| {
        tabs.iter().any(|new| {
            new.id == old.id
                && new.route == old.route
                && new.route_generation == old.route_generation
        })
    })
}
fn refusal_reason(value: &str) -> &'static str {
    match value {
        "engine_busy" => "engine_busy",
        "eligibility_or_scope" => "eligibility_or_scope",
        "carrier_unavailable" => "carrier_unavailable",
        "worker_state_unknown" => "worker_state_unknown",
        "probe_unavailable" => "probe_unavailable",
        "parent_state_changed" => "parent_state_changed",
        "commit_eligibility_changed" => "commit_eligibility_changed",
        "context_quiesce_refused" => "context_quiesce_refused",
        "aborted" => "aborted",
        "document_changed" => "document_changed",
        "renderer_or_native_protected" => "renderer_or_native_protected",
        "beforeunload_veto" => "beforeunload_veto",
        "unexpected_close" => "unexpected_close",
        "native_deadline" => "native_deadline",
        "eligibility_changed" => "eligibility_changed",
        "service_worker_present_or_unknown" => "service_worker_present_or_unknown",
        "freeze_failed" => "freeze_failed",
        "frozen_probe_failed" => "frozen_probe_failed",
        "cookie_capture_unavailable" => "cookie_capture_unavailable",
        "cookie_schema_or_capture_failed" => "cookie_schema_or_capture_failed",
        "native_history" => "native_history",
        "native_loading" => "native_loading",
        "native_private" => "native_private",
        "native_pinned" => "native_pinned",
        "native_frames" => "native_frames",
        "native_post" => "native_post",
        "native_devtools" => "native_devtools",
        "native_download" => "native_download",
        "native_capture" => "native_capture",
        "native_edited" => "native_edited",
        "native_document_unknown" => "native_document_unknown",
        _ => "protected_or_unsupported",
    }
}
fn bytes(snapshot: &Snapshot) -> usize {
    snapshot.sealed.ciphertext.len()
        + snapshot.sealed.nonce.len()
        + snapshot
            .session
            .vkladki
            .iter()
            .map(|t| t.adres.len() + t.zagolovok.len() + 512)
            .sum::<usize>()
        + snapshot.scroll.len() * 64
}
/// Invoked only from the bounded native stdout parser, never renderer/UI IPC.
pub fn receive(app: &tauri::AppHandle, id: &str, generation: u64, frame: Frame) {
    let s: tauri::State<crate::Sostoyanie> = app.state();
    let _write = sessiya::ZAPIS.lock().unwrap();
    let _selection = s.izmenenie_okon.lock().unwrap();
    let pending_views = view_claims().lock().unwrap();
    let selected = s.aktivnaya.lock().ok().and_then(|v| v.clone());
    let mut views = protected_views(&s);
    views.extend(pending_views.keys().cloned());
    let mut map = s.vkladki.lock().unwrap();
    let total = map
        .values()
        .filter_map(|v| v.hibernate.snapshot.as_ref())
        .map(bytes)
        .sum::<usize>();
    let Some(v) = map.get_mut(id).filter(|v| v.zapusk_id == generation) else {
        return;
    };
    let eligible = parent_eligible(
        v,
        id,
        selected.as_deref(),
        &views,
        crate::browser_lock::locked_global(),
    ) && crate::nastrojki::zagruzit().memory_saver;
    let channel = v.kanal.clone();
    match frame {
        Frame::Prepared { snapshot } => {
            if v.hibernate.machine.phase() != Phase::Preparing {
                return;
            }
            let valid = eligible
                && snapshot_valid(&v.hibernate, &snapshot, v.obychnye.as_ref())
                && total.saturating_add(bytes(&snapshot)) <= TOTAL_ESCROW;
            let st = stamp(&snapshot.token);
            let revision = snapshot.token.revision;
            let effects = v.hibernate.machine.apply(Event::Prepared {
                stamp: st,
                eligibility_revision: revision,
                snapshot_valid: valid,
            });
            v.hibernate.effects(effects, channel.as_ref());
            if valid {
                v.hibernate.snapshot = Some(snapshot);
                let effects = v.hibernate.machine.apply(Event::Commit {
                    stamp: st,
                    eligibility_revision: revision,
                });
                v.hibernate.effects(effects, channel.as_ref());
            }
        }
        Frame::FinalSnapshot { snapshot } => {
            if v.hibernate.machine.phase() != Phase::Stopping
                || !snapshot_valid(&v.hibernate, &snapshot, v.obychnye.as_ref())
                || total
                    .saturating_sub(v.hibernate.snapshot.as_ref().map_or(0, bytes))
                    .saturating_add(bytes(&snapshot))
                    > TOTAL_ESCROW
            {
                return;
            }
            if snapshot
                .session
                .sohranit(&crate::brauzer::papka_profilya(id))
                .is_err()
            {
                v.hibernate.reason = Some("session_write_failed");
                return;
            }
            let ack = snapshot.token.clone();
            v.obychnye = Some(snapshot.session.clone());
            v.hibernate.snapshot = Some(snapshot);
            v.hibernate.final_received = true;
            v.hibernate.control(
                channel.as_ref(),
                sessiya::Komanda::HibernateFinalAck { token: ack },
            );
        }
        Frame::Refused { token, reason } => {
            let effects = v.hibernate.machine.apply(Event::Refused {
                stamp: stamp(&token),
                eligibility_revision: token.revision,
            });
            v.hibernate.effects(effects, channel.as_ref());
            if v.hibernate.machine.phase() == Phase::Running {
                v.hibernate.clear_escrow();
                v.hibernate.deadline = None;
                v.hibernate.last_active = Instant::now();
                v.hibernate.reason = Some(refusal_reason(&reason));
                crate::zapisat_v_zhurnal(format!(
                    "{id}: Super sleep refused generation={} attempt={} reason={}",
                    token.generation,
                    token.attempt,
                    refusal_reason(&reason)
                ));
            }
        }
        Frame::AbortAck { token } => {
            let effects = v.hibernate.machine.apply(Event::AbortAck {
                stamp: stamp(&token),
                eligibility_revision: token.revision,
            });
            v.hibernate.effects(effects, channel.as_ref());
            if v.hibernate.machine.phase() == Phase::Running {
                v.hibernate.clear_escrow();
                v.hibernate.deadline = None;
                v.hibernate.reason = None;
            }
        }
        Frame::ShutdownAck { token } => {
            if v.hibernate.final_received {
                let effects = v.hibernate.machine.apply(Event::ShutdownAck {
                    stamp: stamp(&token),
                });
                v.hibernate.effects(effects, channel.as_ref());
            }
        }
        Frame::Restored {
            generation: restored,
        } => {
            if restored == generation
                && v.hibernate.machine.phase() == Phase::Starting
                && v.hibernate.snapshot.is_some()
            {
                v.hibernate.restored_received = true;
                if !v.zapusk_idet && v.okno.is_some() {
                    v.hibernate.ready(generation);
                }
                v.hibernate.last_active = Instant::now();
            }
        }
        Frame::RestoreRefused {
            generation: restored,
        } => {
            if restored == generation {
                v.hibernate.reason = Some("restore_refused");
            }
        }
    }
}
/// One low-frequency parent tick; selected main and ALL extra references protect.
pub fn tick(app: &tauri::AppHandle) {
    if crate::ZAVERSHENIE.load(Ordering::SeqCst) {
        return;
    }
    let s: tauri::State<crate::Sostoyanie> = app.state();
    let _cleanup = cleanup_observation().lock().unwrap();
    let selection_guard = s.izmenenie_okon.lock().unwrap();
    let pending_views = view_claims().lock().unwrap();
    let selected = s.aktivnaya.lock().ok().and_then(|v| v.clone());
    let mut views = protected_views(&s);
    views.extend(pending_views.keys().cloned());
    let settings = crate::nastrojki::zagruzit();
    let locked = crate::browser_lock::locked_global();
    let period = idle_period();
    let now = Instant::now();
    let mut cleanup = Vec::new();
    {
        let Ok(mut map) = s.vkladki.try_lock() else {
            return;
        };
        let total = map
            .values()
            .filter_map(|v| v.hibernate.snapshot.as_ref())
            .map(bytes)
            .sum::<usize>();
        for (id, v) in map.iter_mut() {
            let channel = v.kanal.clone();
            v.hibernate.flush_control(channel.as_ref());
            let eligible = settings.memory_saver
                && parent_eligible(v, id, selected.as_deref(), &views, locked)
                && v.proc
                    .as_ref()
                    .is_some_and(|p| p.hibernation_admission().is_ok());
            if !eligible {
                v.hibernate.invalidate(channel.as_ref());
            } else {
                v.hibernate.eligible = true;
            }
            if locked && !v.hibernate.machine.locked() {
                let effects = v
                    .hibernate
                    .machine
                    .apply(Event::IntentRequested(Intent::Lock));
                v.hibernate.effects(effects, channel.as_ref());
            }
            if !locked && v.hibernate.machine.locked() {
                let effects = v.hibernate.machine.apply(Event::Unlocked);
                v.hibernate.effects(effects, channel.as_ref());
            }
            if v.hibernate.deadline.is_some_and(|deadline| deadline <= now) {
                let st = v.hibernate.machine.stamp();
                let effects = v.hibernate.machine.apply(Event::Deadline { stamp: st });
                v.hibernate.effects(effects, channel.as_ref());
                // No force kill, release or success on a timer. Avoid log/poll storms.
                v.hibernate.deadline = Some(now + Duration::from_secs(60));
            }
            if eligible
                && v.hibernate.machine.phase() == Phase::Running
                && now.duration_since(v.hibernate.last_active) >= period
                && total < TOTAL_ESCROW
            {
                if let (Some(route), Ok(key)) = (v.inherit_route.as_ref(), EphemeralKey::generate())
                {
                    let generation = v.hibernate.machine.stamp().generation;
                    let effects = v.hibernate.machine.apply(Event::PrepareRequested {
                        generation,
                        eligibility_revision: v.hibernate.revision,
                        eligible: true,
                    });
                    if let Some((st, revision)) = effects.iter().find_map(|e| {
                        if let Effect::Prepare {
                            stamp,
                            eligibility_revision,
                        } = e
                        {
                            Some((*stamp, *eligibility_revision))
                        } else {
                            None
                        }
                    }) {
                        let scope = Scope {
                            profile_key: format!("{:x}", Sha256::digest(id.as_bytes())),
                            context_key: route.key.clone(),
                            generation: st.generation,
                            attempt: st.attempt,
                        };
                        let request = Prepare {
                            scope: scope.clone(),
                            revision,
                            key: WireKey::new(key.wire_base64().to_string()),
                        };
                        v.hibernate.scope = Some(scope);
                        v.hibernate.operation_revision = Some(revision);
                        v.hibernate.key = Some(key);
                        v.hibernate.final_received = false;
                        v.hibernate.reason = Some("preparing");
                        v.hibernate.deadline = Some(now + Duration::from_secs(30));
                        if !channel_send(
                            channel.as_ref(),
                            sessiya::Komanda::HibernatePrepare { request },
                        ) {
                            let effects = v.hibernate.machine.apply(Event::Refused {
                                stamp: st,
                                eligibility_revision: revision,
                            });
                            v.hibernate.effects(effects, channel.as_ref());
                            v.hibernate.clear_escrow();
                            v.hibernate.last_active = now;
                            v.hibernate.reason = Some("prepare_channel_failed");
                        }
                    }
                }
            }
            if v.hibernate.machine.phase() == Phase::Stopping {
                if v.proc.as_mut().is_some_and(|owner| {
                    owner.probe() == crate::super_process_owner::ExitProbe::Exited
                }) {
                    let generation = v.hibernate.machine.stamp().generation;
                    let effects = v.hibernate.machine.apply(Event::TreeExited { generation });
                    v.hibernate.effects(effects, channel.as_ref());
                }
                if v.hibernate.release_pending {
                    if let Some(owner) = v.proc.take() {
                        v.kanal = None;
                        v.okno = None;
                        v.zapas_pid = None;
                        cleanup.push((
                            id.clone(),
                            v.zapusk_id,
                            owner,
                            v.nash_proxy.take(),
                            v.tunnel.take(),
                        ));
                    }
                }
            }
        }
    }
    drop(pending_views);
    drop(selection_guard);
    for (id, generation, owner, proxy, tunnel) in cleanup {
        // No replacement can acquire the profile while this native guard is held.
        if let Some(proxy) = proxy {
            proxy.ostanovit();
        }
        if let Some(mut tunnel) = tunnel {
            let _ = tunnel.kill();
            let _ = tunnel.wait();
        }
        crate::release_route_resources(&s, &id, generation);
        match owner.complete() {
            Ok(()) => {
                let wake = {
                    let mut map = s.vkladki.lock().unwrap();
                    let Some(v) = map.get_mut(&id).filter(|v| v.zapusk_id == generation) else {
                        continue;
                    };
                    let st = v.hibernate.machine.stamp();
                    let effects = v
                        .hibernate
                        .machine
                        .apply(Event::LeaseReleased { stamp: st });
                    v.hibernate.effects(effects, None);
                    v.hibernate.release_pending = false;
                    v.hibernate.deadline = None;
                    v.zapusk_idet = false;
                    let sleeping = v.hibernate.machine.successful_hibernation();
                    v.hibernate.reason = if sleeping { Some("idle") } else { None };
                    if sleeping {
                        crate::zapisat_v_zhurnal(format!("{id}: Super sleep generation={generation}, CEF shutdown and owned tree exit confirmed"));
                    }
                    v.hibernate.machine.phase() == Phase::Starting
                };
                let _ = app.emit("vkladki-izmenilis", "");
                if wake {
                    let app = app.clone();
                    let id = id.clone();
                    std::thread::spawn(move || {
                        let _ = tauri::async_runtime::block_on(crate::zapustit_vnutri(
                            &app,
                            &app.state(),
                            &id,
                        ));
                    });
                }
            }
            Err(owner) => {
                let mut map = s.vkladki.lock().unwrap();
                if let Some(v) = map.get_mut(&id).filter(|v| v.zapusk_id == generation) {
                    v.proc = Some(owner);
                    v.hibernate.reason = Some("exit_recheck_unknown");
                } else {
                    std::mem::forget(owner);
                }
            }
        }
    }
}
#[tauri::command]
pub fn super_lifecycle_snapshot(
    window: tauri::WebviewWindow,
    s: tauri::State<'_, crate::Sostoyanie>,
) -> Result<serde_json::Value, String> {
    if window.label() != "main" {
        return Err("Main window required".into());
    }
    let _cleanup = cleanup_observation()
        .lock()
        .map_err(|_| "Lifecycle unavailable")?;
    let mut map = s.vkladki.lock().map_err(|_| "Lifecycle unavailable")?;
    let engines:Vec<_>=map.iter_mut().map(|(id,v)|{
        let st=v.hibernate.machine.stamp();
        let (members,complete)=match v.proc.as_mut(){Some(owner)=>match owner.members(){Ok(list)=>(list,true),Err(_)=>(Vec::new(),false)},None=>(Vec::new(),true)};
        let owned:Vec<_>=members.into_iter().map(|p|serde_json::json!({"pid":p.pid,"birth":p.created.to_string(),"clock":if cfg!(target_os="windows"){"windows_filetime"}else{"linux_ticks"}})).collect();
        let session=v.obychnye.as_ref();
        serde_json::json!({"id":id,"phase":format!("{:?}",v.hibernate.machine.phase()),"generation":st.generation,"attempt":st.attempt,"profile_reserved":v.hibernate.machine.profile_reserved(),"lease_confirmed":v.hibernate.machine.lease_confirmed(),"pid":v.proc.as_ref().and_then(|p|p.pid()),"owned":owned,"owned_complete":complete,"reason":v.hibernate.reason,"tab_count":session.map_or(0,|s|s.vkladki.len()),"live_count":if v.proc.is_some(){session.map_or(0,|s|s.vkladki.iter().filter(|t|!t.otlozhena).count())}else{0}})
    }).collect();
    Ok(serde_json::json!({"schema":1,"engines":engines}))
}
fn view_claims() -> &'static std::sync::Mutex<std::collections::HashMap<String, usize>> {
    static CLAIMS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, usize>>> =
        std::sync::OnceLock::new();
    CLAIMS.get_or_init(Default::default)
}
pub struct ViewClaim(String);
impl ViewClaim {
    pub fn new(id: &str) -> Self {
        *view_claims()
            .lock()
            .unwrap()
            .entry(id.to_owned())
            .or_default() += 1;
        Self(id.to_owned())
    }
}
impl Drop for ViewClaim {
    fn drop(&mut self) {
        let mut claims = view_claims().lock().unwrap();
        if let Some(count) = claims.get_mut(&self.0) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                claims.remove(&self.0);
            }
        }
    }
}
/// Caller holds ViewClaim through attachment; Commit serializes against claims.
pub async fn ensure_view_ready(
    app: &tauri::AppHandle,
    s: &tauri::State<'_, crate::Sostoyanie>,
    id: &str,
) -> Result<(), String> {
    let result = crate::zapustit_vnutri(app, s, id).await;
    if !result.ok {
        return Err(result.soobshchenie);
    }
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        if crate::browser_lock::locked_global() {
            return Err("Браузер заблокирован".into());
        }
        let ready = s.vkladki.lock().unwrap().get(id).is_some_and(|v| {
            v.proc.is_some() && !v.zapusk_idet && v.hibernate.machine.phase() == Phase::Running
        });
        if ready {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("Движок ещё завершает восстановление. Повторите действие.".into());
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn successful_global_unlock_can_start_cold_locked_machine_without_waiting_for_tick() {
        if crate::browser_lock::locked_global() {
            return;
        }
        let mut node = Node::default();
        node.machine.apply(Event::IntentRequested(Intent::Lock));
        assert!(node.machine.locked());
        assert_eq!(node.machine.phase(), Phase::Sleeping);
        assert!(node.begin_start().is_ok());
        assert!(!node.machine.locked());
        assert_eq!(node.machine.phase(), Phase::Starting);
    }
    #[test]
    fn full_real_channel_retries_same_control_without_fabricating_ack() {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        tx.send(sessiya::Komanda::ObnovitInterfeys).unwrap();
        let mut node = Node::default();
        node.control(
            Some(&tx),
            sessiya::Komanda::HibernateFinalAck {
                token: Token {
                    generation: 7,
                    attempt: 9,
                    revision: 11,
                },
            },
        );
        assert!(node.pending_control.is_some());
        assert_eq!(node.machine.phase(), Phase::Sleeping);
        let _ = rx.recv().unwrap();
        node.flush_control(Some(&tx));
        assert!(matches!(
            rx.recv().unwrap(),
            sessiya::Komanda::HibernateFinalAck {
                token: Token {
                    generation: 7,
                    attempt: 9,
                    revision: 11
                }
            }
        ));
        assert!(node.pending_control.is_none());
        node.flush_control(Some(&tx));
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn blocked_and_disconnected_control_delivery_is_bounded_and_never_an_ack() {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        tx.send(sessiya::Komanda::ObnovitInterfeys).unwrap();
        let mut node = Node::default();
        node.control(
            Some(&tx),
            sessiya::Komanda::HibernateAbort {
                token: Token {
                    generation: 1,
                    attempt: 2,
                    revision: 3,
                },
            },
        );
        for _ in 0..31 {
            node.flush_control(Some(&tx));
        }
        assert!(node.pending_control.is_none());
        assert_eq!(node.reason, Some("control_delivery_failed"));
        assert!(!node.machine.successful_hibernation());
        drop(rx);
        node.control(
            Some(&tx),
            sessiya::Komanda::HibernateCommit {
                token: Token {
                    generation: 1,
                    attempt: 2,
                    revision: 3,
                },
            },
        );
        assert!(node.pending_control.is_none());
        assert!(!node.machine.successful_hibernation());
    }
}

/// Observation exclusion covers a short ordered resource-release transaction.
pub(crate) fn cleanup_observation() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(Default::default)
}
