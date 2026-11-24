//! OS profile exclusion and conservative owned-tree exit evidence.
//!
//! INTEGRATION CONTRACT:
//! 1. Acquire ProfileLease BEFORE clearing Chromium Singleton* or spawning. Keep
//!    its stable lock file/directory until release; never unlink/replace the lock.
//! 2. Linux positive ownership uses OwnedCommand::new_current_exe() and
//!    ProcessOwner::spawn_owned(). No caller pre_exec closure can run first.
//!    Its group is created before inherited seccomp denies group/namespace
//!    escape; unknown kernel/architecture fails spawn. Ordinary adopt() remains
//!    unproven. This is lifecycle containment, NOT a replacement for CEF's
//!    security sandbox or protection against external privileged actors.
//! 3. Windows spawn_owned internally adds CREATE_SUSPENDED. With its process
//!    handle pinned and unsignaled it enumerates EXACTLY one owned thread,
//!    pins/verifies its owner+creation identity, assigns a fresh no-breakaway
//!    Job, rechecks identities/liveness, then ResumeThread must return one.
//!    Ordinary adopt remains unproven. No NtResumeProcess is used.
//!    Linux spawn MUST run on a persistent launcher THREAD: PDEATHSIG follows
//!    the creating thread's lifetime, not merely the application PID.
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

/// Builder intentionally hides std::Command: an earlier user pre_exec could
/// fork an escaping writer before our filter. Only this app's own executable
/// can be spawned; CEF/test helper arguments remain the caller's responsibility.
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub struct OwnedCommand {
    command: std::process::Command,
}
#[cfg(any(target_os = "linux", target_os = "windows"))]
impl OwnedCommand {
    pub fn new_current_exe() -> io::Result<Self> {
        Ok(Self {
            command: std::process::Command::new(std::env::current_exe()?),
        })
    }
    pub fn arg(&mut self, value: impl AsRef<std::ffi::OsStr>) -> &mut Self {
        self.command.arg(value);
        self
    }
    pub fn args<I, S>(&mut self, values: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        self.command.args(values);
        self
    }
    pub fn env(
        &mut self,
        key: impl AsRef<std::ffi::OsStr>,
        value: impl AsRef<std::ffi::OsStr>,
    ) -> &mut Self {
        self.command.env(key, value);
        self
    }
    pub fn env_remove(&mut self, key: impl AsRef<std::ffi::OsStr>) -> &mut Self {
        self.command.env_remove(key);
        self
    }
    pub fn current_dir(&mut self, path: impl AsRef<Path>) -> &mut Self {
        self.command.current_dir(path);
        self
    }
    pub fn stdin(&mut self, value: impl Into<std::process::Stdio>) -> &mut Self {
        self.command.stdin(value);
        self
    }
    pub fn stdout(&mut self, value: impl Into<std::process::Stdio>) -> &mut Self {
        self.command.stdout(value);
        self
    }
    pub fn stderr(&mut self, value: impl Into<std::process::Stdio>) -> &mut Self {
        self.command.stderr(value);
        self
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
    /// Sole safe builder origin of containment proof. Linux pre_exec failure
    /// makes std reap its failed child before returning. Windows post-spawn
    /// validation failures return an owner with Unknown admission, keeping the
    /// suspended process and lease for explicit cleanup, never a replacement.
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    pub fn spawn_owned(mut command: OwnedCommand, lease: ProfileLease) -> io::Result<Self> {
        platform::configure(&mut command.command)?;
        let mut child = command.command.spawn()?;
        let tracker = platform::Tracker::contained(&mut child);
        Ok(Self {
            owned: Some(Owned {
                child,
                lease,
                tracker,
            }),
        })
    }
    pub fn pid(&self) -> Option<u32> {
        self.owned.as_ref().map(|o| o.child.id())
    }
    pub fn id(&self) -> u32 {
        self.owned
            .as_ref()
            .expect("owner already completed")
            .child
            .id()
    }
    /// Launcher-only status, NEVER tree-exit or permission to release routes.
    /// Linux observes with waitid(WNOWAIT): Some(status) intentionally keeps
    /// the original leader zombie/PID reserved until the whole tree is done.
    /// No external waitpid(-1), SIGCHLD=SIG_IGN or SA_NOCLDWAIT is permitted.
    pub fn try_wait(&mut self) -> io::Result<Option<std::process::ExitStatus>> {
        let owned = self
            .owned
            .as_mut()
            .ok_or_else(|| io::Error::other("owner completed"))?;
        owned.tracker.try_wait(&mut owned.child)
    }
    /// USER explicit close/startup-failure only. Never invoke for hibernation or
    /// because a timer expired. Err returns the full owner for a retained reaper;
    /// it does NOT release network resources, lease, or fabricate an exit ACK.
    pub fn terminate_explicit(mut self) -> Result<(), Self> {
        let Some(owned) = self.owned.as_mut() else {
            return Err(self);
        };
        if owned.tracker.terminate(&mut owned.child).is_err() {
            return Err(self);
        }
        self.complete()
    }
    pub fn take_stdin(&mut self) -> Option<std::process::ChildStdin> {
        self.owned.as_mut()?.child.stdin.take()
    }
    pub fn take_stdout(&mut self) -> Option<std::process::ChildStdout> {
        self.owned.as_mut()?.child.stdout.take()
    }
    pub fn take_stderr(&mut self) -> Option<std::process::ChildStderr> {
        self.owned.as_mut()?.child.stderr.take()
    }
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
    /// Complete bounded OS membership, including orphans. No ancestry guess,
    /// process arguments or paths. Err means the caller MUST mark its resource
    /// sample incomplete, never interpret it as an empty tree. Observation
    /// updates identities/held handles, but never idle timestamps or lifecycle.
    pub fn members(&mut self) -> Result<Vec<ProcessIdentity>, UnknownReason> {
        self.owned
            .as_mut()
            .ok_or(UnknownReason::ObservationFailed)?
            .tracker
            .members()
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
    // Installed before exec; only plain stack values and async-signal-safe
    // libc calls are used inside pre_exec. No heap/format/logging/locks.
    #[cfg(target_arch = "x86_64")]
    pub fn configure(command: &mut std::process::Command) -> io::Result<()> {
        use std::os::unix::process::CommandExt;
        // An ignored/auto-reaped SIGCHLD would remove the leader reservation
        // behind the owner. This app must not install a generic waitpid(-1)
        // reaper while guarded children exist.
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        if unsafe { libc::sigaction(libc::SIGCHLD, std::ptr::null(), &mut action) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if action.sa_sigaction == libc::SIG_IGN || action.sa_flags & libc::SA_NOCLDWAIT != 0 {
            return Err(io::Error::other("automatic child reaping refused"));
        }
        let parent = unsafe { libc::getpid() };
        unsafe {
            command.pre_exec(move || {
                if libc::setpgid(0, 0) != 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::prctl(
                    libc::PR_SET_PDEATHSIG,
                    libc::SIGKILL as libc::c_ulong,
                    0,
                    0,
                    0,
                ) != 0
                {
                    return Err(io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    return Err(io::Error::from_raw_os_error(libc::ESRCH));
                }
                if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1 as libc::c_ulong, 0, 0, 0) != 0 {
                    return Err(io::Error::last_os_error());
                }
                let filter = containment_filter();
                let program = libc::sock_fprog {
                    len: filter.len() as u16,
                    filter: filter.as_ptr() as *mut libc::sock_filter,
                };
                if libc::prctl(
                    libc::PR_SET_SECCOMP,
                    libc::SECCOMP_MODE_FILTER as libc::c_ulong,
                    &program as *const libc::sock_fprog,
                    0,
                    0,
                ) != 0
                {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Ok(())
    }
    #[cfg(not(target_arch = "x86_64"))]
    pub fn configure(_: &mut std::process::Command) -> io::Result<()> {
        Err(io::Error::from_raw_os_error(libc::ENOTSUP))
    }
    #[cfg(target_arch = "x86_64")]
    fn containment_filter() -> [libc::sock_filter; 21] {
        const LD: u16 = (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16;
        const JEQ: u16 = (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16;
        const JSET: u16 = (libc::BPF_JMP | libc::BPF_JSET | libc::BPF_K) as u16;
        const RET: u16 = (libc::BPF_RET | libc::BPF_K) as u16;
        const AND: u16 = (libc::BPF_ALU | libc::BPF_AND | libc::BPF_K) as u16;
        const fn i(code: u16, jt: u8, jf: u8, k: u32) -> libc::sock_filter {
            libc::sock_filter { code, jt, jf, k }
        }
        [
            i(LD, 0, 0, 4),            // seccomp_data.arch
            i(JEQ, 1, 0, 0xc000_003e), // AUDIT_ARCH_X86_64; reject int80/i386
            i(RET, 0, 0, libc::SECCOMP_RET_KILL_PROCESS),
            i(LD, 0, 0, 0),             // seccomp_data.nr
            i(AND, 0, 0, !0x4000_0000), // x32 ABI cannot bypass matching
            i(JEQ, 0, 1, libc::SYS_setsid as u32),
            i(RET, 0, 0, libc::SECCOMP_RET_ERRNO | libc::EPERM as u32),
            i(JEQ, 0, 1, libc::SYS_setpgid as u32),
            i(RET, 0, 0, libc::SECCOMP_RET_ERRNO | libc::EPERM as u32),
            i(JEQ, 0, 1, libc::SYS_unshare as u32),
            i(RET, 0, 0, libc::SECCOMP_RET_ERRNO | libc::EPERM as u32),
            i(JEQ, 0, 1, libc::SYS_setns as u32),
            i(RET, 0, 0, libc::SECCOMP_RET_ERRNO | libc::EPERM as u32),
            i(JEQ, 0, 1, libc::SYS_clone3 as u32),
            i(RET, 0, 0, libc::SECCOMP_RET_ERRNO | libc::ENOSYS as u32),
            i(JEQ, 0, 4, libc::SYS_clone as u32),
            i(LD, 0, 0, 16),            // low32 args[0] clone flags
            i(JSET, 0, 1, 0x7e02_0080), // every namespace creation flag
            i(RET, 0, 0, libc::SECCOMP_RET_ERRNO | libc::EPERM as u32),
            i(RET, 0, 0, libc::SECCOMP_RET_ALLOW),
            i(RET, 0, 0, libc::SECCOMP_RET_ALLOW),
        ]
    }
    pub struct Tracker {
        root: Option<ProcessIdentity>,
        group: u32,
        known: BTreeMap<u32, u64>,
        failed: bool,
        proven: bool,
        root_exited: bool,
        root_status: Option<std::process::ExitStatus>,
        root_reaped: bool,
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
                proven: false,
                root_exited: false,
                root_status: None,
                root_reaped: false,
            }
        }
        pub fn contained(child: &mut Child) -> Self {
            let mut tracker = Self::adopt(child);
            tracker.proven = true;
            tracker
        }
        fn leader_reserved(&self) -> io::Result<()> {
            if self.root_reaped {
                return Err(io::Error::other("leader reservation released"));
            }
            let root = self
                .root
                .ok_or_else(|| io::Error::other("leader identity missing"))?;
            let raw = std::fs::read_to_string(format!("/proc/{}/stat", root.pid))?;
            if parse_proc_stat(&raw).is_none_or(|s| s.identity != root || s.group != self.group) {
                return Err(io::Error::other("leader reservation changed"));
            }
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            if unsafe { libc::sigaction(libc::SIGCHLD, std::ptr::null(), &mut action) } != 0 {
                return Err(io::Error::last_os_error());
            }
            if action.sa_sigaction == libc::SIG_IGN || action.sa_flags & libc::SA_NOCLDWAIT != 0 {
                return Err(io::Error::other("automatic child reaping refused"));
            }
            Ok(())
        }
        pub fn try_wait(
            &mut self,
            child: &mut Child,
        ) -> io::Result<Option<std::process::ExitStatus>> {
            if self.root_reaped {
                return Ok(self.root_status);
            }
            if let Err(error) = self.leader_reserved() {
                self.failed = true;
                return Err(error);
            }
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            if unsafe {
                libc::waitid(
                    libc::P_PID,
                    child.id(),
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            } != 0
            {
                self.failed = true;
                return Err(io::Error::last_os_error());
            }
            let pid = unsafe { info.si_pid() };
            if pid == 0 {
                return Ok(None);
            }
            if pid as u32 != child.id() {
                self.failed = true;
                return Err(io::Error::other("wait identity mismatch"));
            }
            let status = unsafe { info.si_status() };
            let raw = match info.si_code {
                libc::CLD_EXITED => status << 8,
                libc::CLD_KILLED => status & 0x7f,
                libc::CLD_DUMPED => (status & 0x7f) | 0x80,
                _ => {
                    self.failed = true;
                    return Err(io::Error::other("unexpected wait state"));
                }
            };
            use std::os::unix::process::ExitStatusExt;
            self.root_status = Some(std::process::ExitStatus::from_raw(raw));
            self.root_exited = true;
            Ok(self.root_status)
        }
        pub fn terminate(&mut self, child: &mut Child) -> io::Result<()> {
            if !self.proven {
                return Err(io::Error::other("unproven ownership"));
            }
            // WNOWAIT preserves the original leader PID even after it exits:
            // numeric PGID cannot be recycled into an unrelated new group.
            // Once final probe reaps it, termination is permanently refused.
            self.leader_reserved()?;
            // pidfds additionally prevent individual member PID reuse.
            let entries = std::fs::read_dir("/proc")?;
            let mut signaled = 0usize;
            for (scanned, entry) in entries.enumerate() {
                if scanned >= MAX_PROCESSES {
                    return Err(io::Error::other("process bound"));
                }
                let entry = entry?;
                let Some(pid) = entry
                    .file_name()
                    .to_str()
                    .and_then(|n| n.parse::<u32>().ok())
                else {
                    continue;
                };
                let path = entry.path().join("stat");
                let raw = match std::fs::read_to_string(&path) {
                    Ok(raw) => raw,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                    Err(e) => return Err(e),
                };
                let stat = parse_proc_stat(&raw)
                    .ok_or_else(|| io::Error::other("invalid process identity"))?;
                if stat.group != self.group || matches!(stat.state, b'Z' | b'X' | b'x') {
                    continue;
                }
                signaled += 1;
                if signaled > MAX_OWNED {
                    return Err(io::Error::other("owned process bound"));
                }
                use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
                let raw_fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
                if raw_fd < 0 {
                    let e = io::Error::last_os_error();
                    if e.raw_os_error() == Some(libc::ESRCH) {
                        continue;
                    }
                    return Err(e);
                }
                let fd = unsafe { OwnedFd::from_raw_fd(raw_fd as i32) };
                let after = match std::fs::read_to_string(&path) {
                    Ok(raw) => parse_proc_stat(&raw),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                    Err(e) => return Err(e),
                };
                if after.is_none_or(|s| s.identity != stat.identity || s.group != self.group) {
                    return Err(io::Error::other("process identity changed"));
                }
                if self
                    .known
                    .get(&pid)
                    .is_some_and(|birth| *birth != stat.identity.created)
                {
                    return Err(io::Error::other("process identity reused"));
                }
                self.leader_reserved()?;
                let result = unsafe {
                    libc::syscall(
                        libc::SYS_pidfd_send_signal,
                        fd.as_raw_fd(),
                        libc::SIGKILL,
                        std::ptr::null::<libc::siginfo_t>(),
                        0,
                    )
                };
                if result != 0 {
                    let e = io::Error::last_os_error();
                    if e.raw_os_error() != Some(libc::ESRCH) {
                        return Err(e);
                    }
                }
            }
            // Nonblocking: reaper keeps calling probe; a timeout is never exit.
            let _ = self.try_wait(child)?;
            Ok(())
        }
        pub fn members(&mut self) -> Result<Vec<ProcessIdentity>, UnknownReason> {
            self.admission()?;
            if self.root_reaped {
                // The numeric PGID is no longer reserved. Never attribute a
                // recycled group to this owner, even if its members look valid.
                if unsafe { libc::kill(-(self.group as i32), 0) } == -1
                    && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
                {
                    return Ok(Vec::new());
                }
                return Err(UnknownReason::IdentityUnavailable);
            }
            self.leader_reserved()
                .map_err(|_| UnknownReason::IdentityUnavailable)?;
            let entries =
                std::fs::read_dir("/proc").map_err(|_| UnknownReason::ObservationFailed)?;
            let mut found = Vec::new();
            for (scanned, entry) in entries.enumerate() {
                if scanned >= MAX_PROCESSES {
                    return Err(UnknownReason::ObservationFailed);
                }
                let entry = entry.map_err(|_| UnknownReason::ObservationFailed)?;
                let Some(pid) = entry
                    .file_name()
                    .to_str()
                    .and_then(|s| s.parse::<u32>().ok())
                else {
                    continue;
                };
                // A disappearing entry makes this membership sample incomplete;
                // it is not silently skipped, even when unrelated to this group.
                let raw = std::fs::read_to_string(entry.path().join("stat"))
                    .map_err(|_| UnknownReason::ObservationFailed)?;
                let stat = parse_proc_stat(&raw)
                    .filter(|s| s.identity.pid == pid)
                    .ok_or(UnknownReason::IdentityUnavailable)?;
                if stat.group != self.group {
                    continue;
                }
                if self
                    .known
                    .get(&pid)
                    .is_some_and(|birth| *birth != stat.identity.created)
                {
                    return Err(UnknownReason::IdentityUnavailable);
                }
                if found.len() >= MAX_OWNED {
                    return Err(UnknownReason::ObservationFailed);
                }
                found.push(stat.identity);
            }
            self.leader_reserved()
                .map_err(|_| UnknownReason::IdentityUnavailable)?;
            if !found.iter().any(|p| Some(*p) == self.root) {
                return Err(UnknownReason::IdentityUnavailable);
            }
            // Recheck every PID after collecting the set; never return an
            // identity that already changed or left the owned group.
            for identity in &found {
                let raw = std::fs::read_to_string(format!("/proc/{}/stat", identity.pid))
                    .map_err(|_| UnknownReason::ObservationFailed)?;
                if parse_proc_stat(&raw)
                    .is_none_or(|s| s.identity != *identity || s.group != self.group)
                {
                    return Err(UnknownReason::IdentityUnavailable);
                }
            }
            let additional = found
                .iter()
                .filter(|identity| !self.known.contains_key(&identity.pid))
                .count();
            if self.known.len().saturating_add(additional) > MAX_OWNED {
                return Err(UnknownReason::ObservationFailed);
            }
            for identity in &found {
                self.known.insert(identity.pid, identity.created);
            }
            found.sort_by_key(|p| p.pid);
            Ok(found)
        }
        pub fn identity(&self) -> Option<ProcessIdentity> {
            self.root
        }
        pub fn admission(&self) -> Result<(), UnknownReason> {
            if self.failed {
                return Err(UnknownReason::ObservationFailed);
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
            // Reap only this actual Child. This never constitutes tree proof.
            if self.try_wait(child).is_err() {
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
            // A pinned, exited leader cannot fork. Every descendant is in
            // this still-reserved group. Only after NO live group member is
            // observed may the owner perform its sole actual child reap.
            if !self.root_reaped && self.root_exited && live == 0 {
                if self.leader_reserved().is_err() {
                    self.failed = true;
                    return ExitProbe::Unknown(UnknownReason::IdentityUnavailable);
                }
                match child.try_wait() {
                    Ok(Some(status)) if Some(status) == self.root_status => self.root_reaped = true,
                    _ => {
                        self.failed = true;
                        return ExitProbe::Unknown(UnknownReason::ObservationFailed);
                    }
                }
            }
            // Group may retain non-writer zombies until init reaps them. Wait;
            // ESRCH is still mandatory. After root_reaped no further signaling
            // is allowed, even if this number is later recycled by the kernel.
            let group_empty = if unsafe { libc::kill(-(self.group as i32), 0) } == 0 {
                false
            } else {
                match io::Error::last_os_error().raw_os_error() {
                    Some(libc::ESRCH) => true,
                    _ => return ExitProbe::Unknown(UnknownReason::ObservationFailed),
                }
            };
            evaluate_exit(ExitEvidence {
                containment_proven: self.proven,
                identity_verified: self.root.is_some(),
                observation_complete: true,
                live_processes: if group_empty { live } else { live.max(1) },
                root_exited: self.root_reaped,
            })
        }
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::*;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows::Win32::{
        Foundation::{ERROR_NO_MORE_FILES, FILETIME, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::{Diagnostics::ToolHelp::*, JobObjects::*, Threading::*},
    };
    pub fn configure(command: &mut std::process::Command) -> io::Result<()> {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_SUSPENDED.0);
        Ok(())
    }
    fn thread_id(pid: u32) -> Option<u32> {
        unsafe {
            let raw = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0).ok()?;
            let snapshot = OwnedHandle::from_raw_handle(raw.0);
            let mut entry = THREADENTRY32 {
                dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
                ..Default::default()
            };
            Thread32First(handle(&snapshot), &mut entry).ok()?;
            let mut found = None;
            let mut count = 0usize;
            loop {
                count += 1;
                if count > 32768 {
                    return None;
                }
                if entry.th32OwnerProcessID == pid {
                    if found.is_some() {
                        return None;
                    }
                    found = Some(entry.th32ThreadID);
                }
                entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
                if let Err(error) = Thread32Next(handle(&snapshot), &mut entry) {
                    if error.code() != windows::core::HRESULT::from_win32(ERROR_NO_MORE_FILES.0) {
                        return None;
                    }
                    break;
                }
            }
            found
        }
    }
    fn thread_identity(thread: &OwnedHandle) -> Option<(u32, u64)> {
        unsafe {
            let pid = GetProcessIdOfThread(handle(thread));
            if pid == 0 {
                return None;
            }
            let mut created = FILETIME::default();
            let mut exit = FILETIME::default();
            let mut kernel = FILETIME::default();
            let mut user = FILETIME::default();
            GetThreadTimes(
                handle(thread),
                &mut created,
                &mut exit,
                &mut kernel,
                &mut user,
            )
            .ok()?;
            Some((
                pid,
                ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64,
            ))
        }
    }
    pub struct Tracker {
        root: Option<ProcessIdentity>,
        job: Option<OwnedHandle>,
        thread: Option<OwnedHandle>,
        proven: bool,
        failure: Option<UnknownReason>,
        member_handles: std::collections::BTreeMap<u32, (ProcessIdentity, OwnedHandle)>,
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
    fn process_identity(process: &OwnedHandle, pid: u32) -> Option<ProcessIdentity> {
        unsafe {
            if GetProcessId(handle(process)) != pid
                || WaitForSingleObject(handle(process), 0) != WAIT_TIMEOUT
            {
                return None;
            }
            let mut created = FILETIME::default();
            let mut exit = FILETIME::default();
            let mut kernel = FILETIME::default();
            let mut user = FILETIME::default();
            GetProcessTimes(
                handle(process),
                &mut created,
                &mut exit,
                &mut kernel,
                &mut user,
            )
            .ok()?;
            let created = ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64;
            (created != 0).then_some(ProcessIdentity { pid, created })
        }
    }
    fn job_pids(job: &OwnedHandle) -> Result<Vec<u32>, UnknownReason> {
        #[repr(C)]
        struct List {
            assigned: u32,
            count: u32,
            pids: [usize; 1024],
        }
        let mut list = List {
            assigned: 0,
            count: 0,
            pids: [0; 1024],
        };
        unsafe {
            QueryInformationJobObject(
                handle(job),
                JobObjectBasicProcessIdList,
                &mut list as *mut _ as *mut _,
                std::mem::size_of_val(&list) as u32,
                None,
            )
        }
        .map_err(|_| UnknownReason::ObservationFailed)?;
        if list.count > 1024 || list.assigned != list.count {
            return Err(UnknownReason::ObservationFailed);
        }
        let mut pids = Vec::with_capacity(list.count as usize);
        for &pid in &list.pids[..list.count as usize] {
            let pid = u32::try_from(pid).map_err(|_| UnknownReason::IdentityUnavailable)?;
            if pid == 0 {
                return Err(UnknownReason::IdentityUnavailable);
            }
            pids.push(pid);
        }
        pids.sort_unstable();
        if pids.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(UnknownReason::IdentityUnavailable);
        }
        Ok(pids)
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
        pub fn contained(child: &mut Child) -> Self {
            let mut tracker = Self {
                root: identity(child),
                job: job(),
                thread: None,
                proven: false,
                failure: Some(UnknownReason::IdentityUnavailable),
                member_handles: Default::default(),
            };
            let Some(root) = tracker.root else {
                return tracker;
            };
            if unsafe { GetProcessId(handle(child)) } != root.pid
                || child.try_wait().ok() != Some(None)
                || unsafe { WaitForSingleObject(handle(child), 0) } != WAIT_TIMEOUT
            {
                return tracker;
            }
            let Some(tid) = thread_id(root.pid) else {
                return tracker;
            };
            let Ok(raw) = (unsafe {
                OpenThread(
                    THREAD_QUERY_LIMITED_INFORMATION | THREAD_SUSPEND_RESUME,
                    false,
                    tid,
                )
            }) else {
                return tracker;
            };
            let thread = unsafe { OwnedHandle::from_raw_handle(raw.0) };
            let Some(birth) = thread_identity(&thread) else {
                return tracker;
            };
            if birth.0 != root.pid || birth.1 < root.created || thread_id(root.pid) != Some(tid) {
                return tracker;
            }
            tracker.thread = Some(thread);
            if tracker.job.as_ref().is_none_or(|job| unsafe {
                AssignProcessToJobObject(handle(job), handle(child)).is_err()
            }) {
                tracker.failure = Some(UnknownReason::AssignmentFailed);
                return tracker;
            }
            if identity(child) != Some(root)
                || unsafe { GetProcessId(handle(child)) } != root.pid
                || child.try_wait().ok() != Some(None)
                || unsafe { WaitForSingleObject(handle(child), 0) } != WAIT_TIMEOUT
                || thread_id(root.pid) != Some(tid)
                || thread_identity(tracker.thread.as_ref().unwrap()) != Some(birth)
            {
                return tracker;
            }
            let previous = unsafe { ResumeThread(handle(tracker.thread.as_ref().unwrap())) };
            if previous != 1 {
                tracker.failure = Some(UnknownReason::ResumeFailed);
                return tracker;
            }
            tracker.proven = true;
            tracker.failure = None;
            tracker
        }
        pub fn try_wait(
            &mut self,
            child: &mut Child,
        ) -> io::Result<Option<std::process::ExitStatus>> {
            child.try_wait()
        }
        pub fn terminate(&mut self, child: &mut Child) -> io::Result<()> {
            // USER/startup failure path only; retained Job covers descendants.
            // Even if assignment failed, stop the exact held root handle; probe
            // remains Unknown and retains the lease until containment is proven.
            if let Some(job) = self.job.as_ref() {
                let _ = unsafe { TerminateJobObject(handle(job), 1) };
            }
            match child.kill() {
                Ok(()) => {}
                Err(e) => {
                    if child.try_wait()?.is_none() {
                        return Err(e);
                    }
                }
            }
            let _ = child.try_wait()?;
            Ok(())
        }
        pub fn adopt(child: &Child) -> Self {
            let mut tracker = Self {
                root: identity(child),
                job: job(),
                thread: None,
                proven: false,
                failure: None,
                member_handles: Default::default(),
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
                member_handles: Default::default(),
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
        pub fn members(&mut self) -> Result<Vec<ProcessIdentity>, UnknownReason> {
            self.admission()?;
            let job = self.job.as_ref().ok_or(UnknownReason::ObservationFailed)?;
            let pids = job_pids(job)?;
            self.member_handles.retain(|pid, (_, process)| {
                pids.binary_search(pid).is_ok()
                    && unsafe { WaitForSingleObject(handle(process), 0) } == WAIT_TIMEOUT
            });
            let mut result = Vec::with_capacity(pids.len());
            for &pid in &pids {
                if !self.member_handles.contains_key(&pid) {
                    let raw = unsafe {
                        OpenProcess(
                            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                            false,
                            pid,
                        )
                    }
                    .map_err(|_| UnknownReason::IdentityUnavailable)?;
                    let process = unsafe { OwnedHandle::from_raw_handle(raw.0) };
                    let identity = process_identity(&process, pid)
                        .ok_or(UnknownReason::IdentityUnavailable)?;
                    let mut inside = windows::Win32::Foundation::BOOL(0);
                    unsafe { IsProcessInJob(handle(&process), handle(job), &mut inside) }
                        .map_err(|_| UnknownReason::ObservationFailed)?;
                    if !inside.as_bool() {
                        return Err(UnknownReason::IdentityUnavailable);
                    }
                    self.member_handles.insert(pid, (identity, process));
                }
                let (identity, process) = self.member_handles.get(&pid).unwrap();
                if process_identity(process, pid) != Some(*identity) {
                    return Err(UnknownReason::IdentityUnavailable);
                }
                let mut inside = windows::Win32::Foundation::BOOL(0);
                unsafe { IsProcessInJob(handle(process), handle(job), &mut inside) }
                    .map_err(|_| UnknownReason::ObservationFailed)?;
                if !inside.as_bool() {
                    return Err(UnknownReason::IdentityUnavailable);
                }
                result.push(*identity);
            }
            // Churn is an incomplete sample, not a partial success. The Job is
            // retained and verified process handles stay pinned across reads.
            if job_pids(job)? != pids {
                return Err(UnknownReason::ObservationFailed);
            }
            Ok(result)
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
        pub fn try_wait(
            &mut self,
            child: &mut Child,
        ) -> io::Result<Option<std::process::ExitStatus>> {
            child.try_wait()
        }
        pub fn terminate(&mut self, _: &mut Child) -> io::Result<()> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "process owner unsupported",
            ))
        }
        pub fn members(&mut self) -> Result<Vec<ProcessIdentity>, UnknownReason> {
            Err(UnknownReason::Unsupported)
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
