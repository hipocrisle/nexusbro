//! OS profile exclusion and conservative owned-tree exit evidence.
//!
//! INTEGRATION CONTRACT:
//! 1. Acquire ProfileLease BEFORE clearing Chromium Singleton* or spawning. Keep
//!    its stable lock file/directory until release; never unlink/replace the lock.
//! 2. Linux Command::process_group(0), existing PDEATHSIG, NO daemon/setsid are
//!    required for observation. A process group is NOT secure containment:
//!    adoption refuses hibernation admission and never reports Exited for it.
//!    A future proven cgroup/pidfd lifecycle adapter is required to lift this.
//! 3. Windows normal Child adoption is post-spawn and cannot exclude assignment
//!    races. For proof, native CreateProcessW(CREATE_SUSPENDED) must return Child
//!    plus its ORIGINAL primary-thread OwnedHandle. unsafe adopt_suspended checks
//!    its PID, assigns a fresh no-breakaway Job, then ResumeThread; count must be
//!    exactly one. Never use NtResumeProcess or discover arbitrary thread IDs.
//!    Existing std Command::spawn does NOT expose the primary-thread handle, so
//!    cannot satisfy this API by merely adding CREATE_SUSPENDED.
//! 4. Send graceful stop via existing IPC. Poll probe, never use timeout/EOF,
//!    window disappearance, Child::try_wait alone, or kill as exit evidence.
//!    Only complete() after Exited releases the lease; failed complete returns
//!    the entire owner. Drop of an active/unproven owner intentionally LEAKS its
//!    handles+lease until process exit; it never unlocks for a replacement.
//! 5. This guard is per canonical native profile. It does not replace the pure
//!    generation/attempt machine, cookie/flush/Shutdown ACKs, or resource order.
use std::{
    fs::{File, OpenOptions},
    io,
    path::Path,
    process::Child,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub created: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnknownReason {
    ContainmentUnproven,
    IdentityUnavailable,
    ObservationFailed,
    AssignmentFailed,
    ResumeFailed,
    Unsupported,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitProbe {
    Alive { observed: usize },
    Exited,
    Unknown(UnknownReason),
}

/// Pure admission gate, also exercised by the small harness. Observation of an
/// empty group/launcher is deliberately insufficient without containment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExitEvidence {
    pub containment_proven: bool,
    pub identity_verified: bool,
    pub observation_complete: bool,
    pub live_processes: usize,
    pub root_exited: bool,
}
pub fn evaluate_exit(evidence: ExitEvidence) -> ExitProbe {
    if !evidence.observation_complete {
        return ExitProbe::Unknown(UnknownReason::ObservationFailed);
    }
    if !evidence.identity_verified {
        return ExitProbe::Unknown(UnknownReason::IdentityUnavailable);
    }
    if evidence.live_processes > 0 {
        return ExitProbe::Alive {
            observed: evidence.live_processes,
        };
    }
    if !evidence.containment_proven {
        return ExitProbe::Unknown(UnknownReason::ContainmentUnproven);
    }
    if !evidence.root_exited {
        return ExitProbe::Alive { observed: 1 };
    }
    ExitProbe::Exited
}

/// Kernel lock, not a PID file. Does not contain profile/secret data.
/// A standalone lease can be dropped before spawn; once adopted, Owner Drop
/// prevents its accidental release while any writer might remain.
pub struct ProfileLease {
    file: File,
}
impl std::fmt::Debug for ProfileLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProfileLease([locked])")
    }
}
impl ProfileLease {
    pub fn acquire(profile: &Path) -> io::Result<Self> {
        let canonical = profile.canonicalize()?;
        if !canonical.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "profile is not a directory",
            ));
        }
        let path = canonical.join(".nexusbro-profile-lease");
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(0).custom_flags(0x0020_0000); /* FILE_FLAG_OPEN_REPARSE_POINT */
        }
        let file = options.open(path)?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid lease file",
            ));
        }
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::fs::MetadataExt;
            if file.metadata()?.file_attributes() & 0x400 != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "reparse lease refused",
                ));
            }
        }
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "profile lease unsupported",
        ));
        Ok(Self { file })
    }
}

struct Owned {
    child: Child,
    lease: ProfileLease,
    tracker: platform::Tracker,
}
pub struct ProcessOwner {
    owned: Option<Owned>,
}
impl std::fmt::Debug for ProcessOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProcessOwner([retained])")
    }
}
impl ProcessOwner {
    /// Observes a running child. No post-spawn assignment is containment proof.
    pub fn adopt(child: Child, lease: ProfileLease) -> Self {
        let tracker = platform::Tracker::adopt(&child);
        Self {
            owned: Some(Owned {
                child,
                lease,
                tracker,
            }),
        }
    }
    /// # Safety
    /// Child and thread MUST come from the same fresh CREATE_SUSPENDED native
    /// CreateProcess call and the thread must NEVER have run/resumed before.
    /// The thread must be its original primary thread, not merely a same-PID
    /// suspended worker. The child must not already own/escape another profile.
    #[cfg(target_os = "windows")]
    pub unsafe fn adopt_suspended(
        child: Child,
        primary_thread: std::os::windows::io::OwnedHandle,
        lease: ProfileLease,
    ) -> Self {
        let tracker = unsafe { platform::Tracker::suspended(&child, primary_thread) };
        Self {
            owned: Some(Owned {
                child,
                lease,
                tracker,
            }),
        }
    }
    pub fn identity(&self) -> Option<ProcessIdentity> {
        self.owned.as_ref().and_then(|o| o.tracker.identity())
    }
    pub fn hibernation_admission(&self) -> Result<(), UnknownReason> {
        self.owned
            .as_ref()
            .ok_or(UnknownReason::ObservationFailed)?
            .tracker
            .admission()
    }
    pub fn probe(&mut self) -> ExitProbe {
        let Some(owned) = self.owned.as_mut() else {
            return ExitProbe::Unknown(UnknownReason::ObservationFailed);
        };
        let result = owned.tracker.probe(&mut owned.child);
        result
    }
    /// Release only after a fresh complete-tree check; no cached timeout proof.
    /// Err returns ownership intact. No kill, retry loop or network teardown.
    pub fn complete(mut self) -> Result<(), Self> {
        if self.probe() != ExitProbe::Exited {
            return Err(self);
        }
        drop(self.owned.take());
        Ok(())
    }
}
impl Drop for ProcessOwner {
    fn drop(&mut self) {
        if let Some(owned) = self.owned.take() {
            // Dropping Child does not wait; dropping a live Job can kill its
            // members and dropping the File unlocks. Preserve ALL of them.
            std::mem::forget(owned);
            eprintln!("profile process owner retained: exit not explicitly completed");
        }
    }
}

/// Linux /proc stat comm may contain whitespace and closing parentheses. Field
/// positions are relative to the LAST ')' delimiter, not split_whitespace on
/// the entire line. Unknown/truncated/negative/zero identity is refused.
#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcStat {
    pub identity: ProcessIdentity,
    pub parent: u32,
    pub group: u32,
    pub session: u32,
    pub state: u8,
}
#[cfg(any(target_os = "linux", test))]
pub fn parse_proc_stat(input: &str) -> Option<ProcStat> {
    let open = input.find('(')?;
    let close = input.rfind(')')?;
    if close <= open {
        return None;
    }
    let pid = input[..open].trim().parse::<u32>().ok()?;
    let rest = input
        .get(close + 1..)?
        .split_whitespace()
        .collect::<Vec<_>>();
    let state = *rest.first()?.as_bytes().first()?;
    if rest[0].len() != 1 || !b"RSDZTWtXxKWPIN".contains(&state) {
        return None;
    }
    let parent = rest.get(1)?.parse().ok()?;
    let group = rest.get(2)?.parse().ok()?;
    let session = rest.get(3)?.parse().ok()?;
    let created = rest.get(19)?.parse().ok()?;
    if pid == 0 || created == 0 {
        return None;
    }
    Some(ProcStat {
        identity: ProcessIdentity { pid, created },
        parent,
        group,
        session,
        state,
    })
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use std::collections::BTreeMap;
    const MAX_PROCESSES: usize = 32768;
    const MAX_OWNED: usize = 1024;
    pub struct Tracker {
        root: Option<ProcessIdentity>,
        group: u32,
        known: BTreeMap<u32, u64>,
        failed: bool,
    }
    impl Tracker {
        pub fn adopt(child: &Child) -> Self {
            let stat = std::fs::read_to_string(format!("/proc/{}/stat", child.id()))
                .ok()
                .and_then(|s| parse_proc_stat(&s));
            let root = stat.map(|s| s.identity);
            let mut known = BTreeMap::new();
            if let Some(root) = root {
                known.insert(root.pid, root.created);
            }
            Self {
                root,
                group: child.id(),
                known,
                failed: stat.is_none_or(|s| s.group != child.id()),
            }
        }
        pub fn identity(&self) -> Option<ProcessIdentity> {
            self.root
        }
        pub fn admission(&self) -> Result<(), UnknownReason> {
            Err(if self.root.is_none() {
                UnknownReason::IdentityUnavailable
            } else {
                UnknownReason::ContainmentUnproven
            })
        }
        pub fn probe(&mut self, child: &mut Child) -> ExitProbe {
            // Reap only this actual Child. This never constitutes tree proof.
            if child.try_wait().is_err() {
                self.failed = true;
            }
            let Ok(entries) = std::fs::read_dir("/proc") else {
                return ExitProbe::Unknown(UnknownReason::ObservationFailed);
            };
            let mut all = BTreeMap::new();
            let mut scanned = 0;
            for entry in entries {
                let Ok(entry) = entry else {
                    self.failed = true;
                    continue;
                };
                let Some(pid) = entry
                    .file_name()
                    .to_str()
                    .and_then(|n| n.parse::<u32>().ok())
                else {
                    continue;
                };
                scanned += 1;
                if scanned > MAX_PROCESSES {
                    return ExitProbe::Unknown(UnknownReason::ObservationFailed);
                }
                match std::fs::read_to_string(entry.path().join("stat")) {
                    Ok(raw) => match parse_proc_stat(&raw) {
                        Some(s) if s.identity.pid == pid => {
                            all.insert(pid, s);
                        }
                        _ => self.failed = true,
                    },
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(_) => self.failed = true,
                }
            }
            // Learn group and descendants while preserving creation identities;
            // a recycled numeric PID is never silently attached to this owner.
            loop {
                let mut changed = false;
                for (&pid, stat) in &all {
                    let descendant = self.known.get(&stat.parent).is_some_and(|birth| {
                        all.get(&stat.parent)
                            .is_some_and(|p| p.identity.created == *birth)
                    });
                    if stat.group == self.group || descendant {
                        match self.known.get(&pid) {
                            Some(birth) if *birth != stat.identity.created => self.failed = true,
                            None => {
                                if self.known.len() >= MAX_OWNED {
                                    return ExitProbe::Unknown(UnknownReason::ObservationFailed);
                                }
                                self.known.insert(pid, stat.identity.created);
                                changed = true;
                            }
                            _ => {}
                        }
                    }
                }
                if !changed {
                    break;
                }
            }
            let live = self
                .known
                .iter()
                .filter(|(pid, birth)| {
                    all.get(pid).is_some_and(|s| {
                        s.identity.created == **birth && !matches!(s.state, b'Z' | b'X' | b'x')
                    })
                })
                .count();
            if self.failed {
                return ExitProbe::Unknown(UnknownReason::ObservationFailed);
            }
            // Even an empty group cannot prove absence of an unobserved setsid
            // descendant. Do not release the lock on that incomplete evidence.
            evaluate_exit(ExitEvidence {
                containment_proven: false,
                identity_verified: self.root.is_some(),
                observation_complete: true,
                live_processes: live,
                root_exited: false,
            })
        }
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::*;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows::Win32::{
        Foundation::{FILETIME, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::{JobObjects::*, Threading::*},
    };
    pub struct Tracker {
        root: Option<ProcessIdentity>,
        job: Option<OwnedHandle>,
        thread: Option<OwnedHandle>,
        proven: bool,
        failure: Option<UnknownReason>,
    }
    fn handle(value: &impl AsRawHandle) -> HANDLE {
        HANDLE(value.as_raw_handle())
    }
    fn identity(child: &Child) -> Option<ProcessIdentity> {
        unsafe {
            let mut created = FILETIME::default();
            let mut exit = FILETIME::default();
            let mut kernel = FILETIME::default();
            let mut user = FILETIME::default();
            GetProcessTimes(
                handle(child),
                &mut created,
                &mut exit,
                &mut kernel,
                &mut user,
            )
            .ok()?;
            Some(ProcessIdentity {
                pid: child.id(),
                created: ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64,
            })
        }
    }
    fn job() -> Option<OwnedHandle> {
        unsafe {
            let raw = CreateJobObjectW(None, None).ok()?;
            let owned = OwnedHandle::from_raw_handle(raw.0);
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            // No BREAKAWAY_OK/SILENT_BREAKAWAY_OK. Handle is retained until actual
            // exit; it is never closed as a timeout/normal stopping mechanism.
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                handle(&owned),
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of_val(&limits) as u32,
            )
            .ok()?;
            Some(owned)
        }
    }
    impl Tracker {
        pub fn adopt(child: &Child) -> Self {
            let mut tracker = Self {
                root: identity(child),
                job: job(),
                thread: None,
                proven: false,
                failure: None,
            };
            if tracker.job.as_ref().is_none_or(|job| unsafe {
                AssignProcessToJobObject(handle(job), handle(child)).is_err()
            }) {
                tracker.failure = Some(UnknownReason::AssignmentFailed);
            }
            tracker
        }
        pub unsafe fn suspended(child: &Child, thread: OwnedHandle) -> Self {
            let mut tracker = Self {
                root: identity(child),
                job: job(),
                thread: Some(thread),
                proven: false,
                failure: None,
            };
            let correct = unsafe { GetProcessIdOfThread(handle(tracker.thread.as_ref().unwrap())) }
                == child.id();
            if !correct || tracker.root.is_none() {
                tracker.failure = Some(UnknownReason::IdentityUnavailable);
                return tracker;
            }
            let assigned = tracker.job.as_ref().is_some_and(|job| unsafe {
                AssignProcessToJobObject(handle(job), handle(child)).is_ok()
            });
            if !assigned {
                tracker.failure = Some(UnknownReason::AssignmentFailed);
                return tracker;
            }
            let previous = unsafe { ResumeThread(handle(tracker.thread.as_ref().unwrap())) };
            if previous != 1 {
                tracker.failure = Some(UnknownReason::ResumeFailed);
                return tracker;
            }
            tracker.proven = true;
            tracker
        }
        pub fn identity(&self) -> Option<ProcessIdentity> {
            self.root
        }
        pub fn admission(&self) -> Result<(), UnknownReason> {
            if let Some(reason) = self.failure {
                return Err(reason);
            }
            if self.root.is_none() {
                return Err(UnknownReason::IdentityUnavailable);
            }
            if self.proven {
                Ok(())
            } else {
                Err(UnknownReason::ContainmentUnproven)
            }
        }
        pub fn probe(&mut self, child: &mut Child) -> ExitProbe {
            if let Err(reason) = self.admission() {
                return ExitProbe::Unknown(reason);
            }
            if identity(child) != self.root {
                return ExitProbe::Unknown(UnknownReason::IdentityUnavailable);
            }
            let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
            let Some(job) = self.job.as_ref() else {
                return ExitProbe::Unknown(UnknownReason::ObservationFailed);
            };
            if unsafe {
                QueryInformationJobObject(
                    handle(job),
                    JobObjectBasicAccountingInformation,
                    &mut accounting as *mut _ as *mut _,
                    std::mem::size_of_val(&accounting) as u32,
                    None,
                )
            }
            .is_err()
            {
                return ExitProbe::Unknown(UnknownReason::ObservationFailed);
            }
            if accounting.ActiveProcesses > 0 {
                return ExitProbe::Alive {
                    observed: accounting.ActiveProcesses as usize,
                };
            }
            match unsafe { WaitForSingleObject(handle(child), 0) } {
                WAIT_OBJECT_0 => match child.try_wait() {
                    Ok(Some(_)) => ExitProbe::Exited,
                    _ => ExitProbe::Unknown(UnknownReason::ObservationFailed),
                },
                WAIT_TIMEOUT => ExitProbe::Alive { observed: 1 },
                _ => ExitProbe::Unknown(UnknownReason::ObservationFailed),
            }
        }
    }
}
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod platform {
    use super::*;
    pub struct Tracker;
    impl Tracker {
        pub fn adopt(_: &Child) -> Self {
            Self
        }
        pub fn identity(&self) -> Option<ProcessIdentity> {
            None
        }
        pub fn admission(&self) -> Result<(), UnknownReason> {
            Err(UnknownReason::Unsupported)
        }
        pub fn probe(&mut self, _: &mut Child) -> ExitProbe {
            ExitProbe::Unknown(UnknownReason::Unsupported)
        }
    }
}
