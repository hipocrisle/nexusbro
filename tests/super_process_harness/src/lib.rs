#[path = "../../../desktop/src-tauri/src/super_process_owner.rs"]
pub mod owner;

#[cfg(test)]
mod tests {
    use super::owner::*;
    fn stat(comm: &str, state: &str, pid: &str, created: &str) -> String {
        // field3 state,4 parent,5 group,6 session ... field22 starttime.
        let mut fields = vec![state, "1", "42", "42"];
        fields.extend(std::iter::repeat("0").take(15));
        fields.push(created);
        format!("{pid} ({comm}) {} 0 0 0", fields.join(" "))
    }
    #[test]
    fn linux_stat_comm_closing_parens_and_spaces_preserve_identity() {
        let parsed = parse_proc_stat(&stat("worker ) name ( )", "S", "42", "987654321")).unwrap();
        assert_eq!(
            parsed.identity,
            ProcessIdentity {
                pid: 42,
                created: 987654321
            }
        );
        assert_eq!(
            (parsed.parent, parsed.group, parsed.session, parsed.state),
            (1, 42, 42, b'S')
        );
    }
    #[test]
    fn malformed_stat_never_supplies_an_identity() {
        for line in [
            "",
            "42 worker S 1",
            "42 (unfinished",
            "0 (x) S 1",
            "42 () S",
            "-1 (x) S",
        ] {
            assert!(parse_proc_stat(line).is_none());
        }
        for (pid, birth, state) in [
            ("0", "10", "S"),
            ("-2", "10", "S"),
            ("42", "0", "S"),
            ("42", "-1", "S"),
            ("42", "overflow", "S"),
            ("42", "10", "??"),
            ("42", "10", "?"),
        ] {
            assert!(parse_proc_stat(&stat("x", state, pid, birth)).is_none());
        }
    }
    #[test]
    fn creation_identity_distinguishes_reused_numeric_pid() {
        let first = parse_proc_stat(&stat("cef", "S", "42", "101")).unwrap();
        let next = parse_proc_stat(&stat("cef", "S", "42", "202")).unwrap();
        assert_eq!(first.identity.pid, next.identity.pid);
        assert_ne!(first.identity, next.identity);
    }
    #[test]
    fn zombie_state_is_not_treated_as_a_live_runnable_process() {
        for state in ["Z", "X", "x"] {
            assert_eq!(
                parse_proc_stat(&stat("cef", state, "42", "123"))
                    .unwrap()
                    .state,
                state.as_bytes()[0]
            );
        }
    }
    #[test]
    fn every_exit_evidence_combination_fails_closed() {
        for containment_proven in [false, true] {
            for identity_verified in [false, true] {
                for observation_complete in [false, true] {
                    for root_exited in [false, true] {
                        for live_processes in [0, 1, 20] {
                            let result = evaluate_exit(ExitEvidence {
                                containment_proven,
                                identity_verified,
                                observation_complete,
                                root_exited,
                                live_processes,
                            });
                            assert_eq!(
                                result == ExitProbe::Exited,
                                containment_proven
                                    && identity_verified
                                    && observation_complete
                                    && root_exited
                                    && live_processes == 0
                            );
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn empty_process_group_or_launcher_exit_cannot_unlock() {
        assert_eq!(
            evaluate_exit(ExitEvidence {
                containment_proven: false,
                identity_verified: true,
                observation_complete: true,
                live_processes: 0,
                root_exited: true
            }),
            ExitProbe::Unknown(UnknownReason::ContainmentUnproven)
        );
        assert_eq!(
            evaluate_exit(ExitEvidence {
                containment_proven: true,
                identity_verified: true,
                observation_complete: true,
                live_processes: 1,
                root_exited: true
            }),
            ExitProbe::Alive { observed: 1 }
        );
    }
    #[test]
    fn permission_or_identity_failure_wins_over_empty_tree() {
        for (identity_verified, observation_complete) in
            [(false, true), (true, false), (false, false)]
        {
            assert!(matches!(
                evaluate_exit(ExitEvidence {
                    containment_proven: true,
                    identity_verified,
                    observation_complete,
                    live_processes: 0,
                    root_exited: true
                }),
                ExitProbe::Unknown(_)
            ));
        }
    }
}

// Native process tests must run on the disposable GitHub Linux runner ONLY.
// Do not execute this harness on DE-1. All fixtures re-exec this test binary;
// no shell/external executable, force-kill or production profile is involved.
#[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
mod native_linux_tests {
    use super::owner::*;
    use std::{
        io::{BufRead, BufReader, Read},
        path::PathBuf,
        process::Stdio,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };
    fn profile() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "nexusbro-owned-fixture-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        dir
    }
    fn command(mode: &str) -> OwnedCommand {
        let mut command = OwnedCommand::new_current_exe().unwrap();
        command
            .args([
                "--exact",
                "native_linux_tests::contained_fixture",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("NB_OWNED_FIXTURE", mode)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        command
    }
    fn exited(owner: &mut ProcessOwner) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            match owner.probe() {
                ExitProbe::Exited => return,
                ExitProbe::Unknown(reason) => panic!("owned tree probe unavailable: {reason:?}"),
                ExitProbe::Alive { .. } => {}
            }
            assert!(
                Instant::now() < deadline,
                "owned fixture did not gracefully exit"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    fn denied(result: libc::c_long) {
        assert_eq!(result, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::EPERM)
        );
    }
    fn assert_denials() {
        unsafe {
            denied(libc::setsid() as libc::c_long);
            denied(libc::setpgid(0, 0) as libc::c_long);
            denied(libc::unshare(libc::CLONE_NEWUSER) as libc::c_long);
            denied(libc::setns(-1, 0) as libc::c_long);
            denied(libc::syscall(
                libc::SYS_clone,
                (libc::CLONE_NEWUSER | libc::SIGCHLD) as libc::c_ulong,
                0,
                0,
                0,
                0,
            ));
            assert_eq!(libc::syscall(libc::SYS_clone3, 0, 0), -1);
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ENOSYS)
            );
            denied(libc::syscall(
                (libc::SYS_setsid as u64 | 0x4000_0000) as libc::c_long,
            ));
        }
    }
    #[test]
    fn contained_fixture() {
        let Ok(mode) = std::env::var("NB_OWNED_FIXTURE") else {
            return;
        };
        match mode.as_str() {
            "denial" => {
                assert_denials();
                // exec of a later descendant inherits the same restrictions.
                let status = std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "native_linux_tests::contained_fixture",
                        "--nocapture",
                        "--test-threads=1",
                    ])
                    .env("NB_OWNED_FIXTURE", "leaf-denial")
                    .status()
                    .unwrap();
                assert!(status.success());
                println!("DENIAL_OK");
            }
            "leaf-denial" => {
                assert_denials();
                println!("EXEC_INHERIT_OK");
            }
            "tree" => unsafe {
                let mut ids = [-1; 2];
                assert_eq!(libc::pipe2(ids.as_mut_ptr(), libc::O_CLOEXEC), 0);
                let child = libc::fork();
                assert!(child >= 0);
                if child == 0 {
                    libc::close(ids[0]);
                    let grandchild = libc::fork();
                    if grandchild < 0 {
                        libc::_exit(91)
                    }
                    if grandchild == 0 {
                        // Only libc calls in post-fork grandchild. Failure to
                        // deny setsid exits nonzero and the parent detects EOF.
                        if libc::setsid() != -1 || *libc::__errno_location() != libc::EPERM {
                            libc::_exit(92)
                        }
                        if libc::setpgid(0, 0) != -1 || *libc::__errno_location() != libc::EPERM {
                            libc::_exit(93)
                        }
                        let pid = libc::getpid();
                        if libc::write(
                            ids[1],
                            &pid as *const _ as *const _,
                            std::mem::size_of_val(&pid),
                        ) != std::mem::size_of_val(&pid) as isize
                        {
                            libc::_exit(94)
                        }
                        libc::close(ids[1]);
                        let mut byte = 0u8;
                        loop {
                            let n = libc::read(0, &mut byte as *mut _ as *mut _, 1);
                            if n >= 0 {
                                break;
                            }
                            if *libc::__errno_location() != libc::EINTR {
                                libc::_exit(95)
                            }
                        }
                        libc::_exit(0)
                    }
                    libc::_exit(0)
                }
                libc::close(ids[1]);
                let mut grandchild = 0i32;
                assert_eq!(
                    libc::read(
                        ids[0],
                        &mut grandchild as *mut _ as *mut _,
                        std::mem::size_of_val(&grandchild)
                    ),
                    std::mem::size_of_val(&grandchild) as isize
                );
                libc::close(ids[0]);
                assert!(grandchild > 0);
                let mut status = 0;
                assert_eq!(libc::waitpid(child, &mut status, 0), child);
                println!(
                    "GRANDCHILD {grandchild} GROUP {}",
                    libc::getpgid(grandchild)
                );
                use std::io::Write;
                std::io::stdout().flush().unwrap();
                // Root really exits while grandchild waits for the owner's
                // stdin EOF. There is no sleep approximating a root exit.
                libc::_exit(0)
            },
            _ => panic!("unknown owned fixture mode"),
        }
    }
    #[test]
    fn seccomp_denies_escape_in_root_and_exec_descendant() {
        let dir = profile();
        let lease = ProfileLease::acquire(&dir).unwrap();
        let mut owner = ProcessOwner::spawn_owned(command("denial"), lease).unwrap();
        assert_eq!(owner.hibernation_admission(), Ok(()));
        let mut output = String::new();
        owner
            .take_stdout()
            .unwrap()
            .read_to_string(&mut output)
            .unwrap();
        assert!(output.contains("DENIAL_OK"));
        assert!(output.contains("EXEC_INHERIT_OK"));
        exited(&mut owner);
        owner.complete().unwrap();
        drop(ProfileLease::acquire(&dir).unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn root_exit_with_live_grandchild_holds_lease_until_real_group_exit() {
        let dir = profile();
        let lease = ProfileLease::acquire(&dir).unwrap();
        let mut owner = ProcessOwner::spawn_owned(command("tree"), lease).unwrap();
        assert_eq!(owner.hibernation_admission(), Ok(()));
        let root = owner.identity().unwrap();
        let input = owner.take_stdin().unwrap();
        let mut output = BufReader::new(owner.take_stdout().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            assert!(output.read_line(&mut line).unwrap() > 0);
            if line.contains("GRANDCHILD ") {
                break;
            }
        }
        let words = line.split_whitespace().collect::<Vec<_>>();
        let at = words.iter().position(|s| *s == "GRANDCHILD").unwrap();
        let grandchild = words[at + 1].parse::<u32>().unwrap();
        let group = words[at + 3].parse::<u32>().unwrap();
        assert_eq!(group, root.pid);
        let deadline = Instant::now() + Duration::from_secs(10);
        while std::path::Path::new(&format!("/proc/{}/stat", root.pid)).exists() {
            assert!(matches!(owner.probe(), ExitProbe::Alive { .. }));
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(std::path::Path::new(&format!("/proc/{grandchild}/stat")).exists());
        assert!(matches!(owner.probe(), ExitProbe::Alive { .. }));
        assert!(ProfileLease::acquire(&dir).is_err());
        owner = owner.complete().unwrap_err();
        assert!(ProfileLease::acquire(&dir).is_err());
        drop(input); // Graceful EOF, not a signal/kill/timeout shortcut.
        exited(&mut owner);
        owner.complete().unwrap();
        drop(ProfileLease::acquire(&dir).unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

// Disposable Windows GitHub runner only. Re-exec the same fixture binary;
// verify Job inheritance and actual descendant liveness after launcher exit.
#[cfg(all(test, target_os = "windows"))]
mod native_windows_tests {
    use super::owner::*;
    use std::{
        io::{BufRead, BufReader, Read, Write},
        os::windows::process::CommandExt,
        path::PathBuf,
        process::Stdio,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };
    use windows::Win32::System::Threading::CREATE_BREAKAWAY_FROM_JOB;
    fn profile() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "nexusbro-owned-job-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        dir
    }
    fn command() -> OwnedCommand {
        let mut command = OwnedCommand::new_current_exe().unwrap();
        command
            .args([
                "--exact",
                "native_windows_tests::job_fixture",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("NB_OWNED_JOB_FIXTURE", "parent")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        command
    }
    fn launcher_exited(owner: &mut ProcessOwner) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if owner.try_wait().unwrap().is_some() {
                return;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    fn exited(owner: &mut ProcessOwner) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            match owner.probe() {
                ExitProbe::Exited => return,
                ExitProbe::Unknown(reason) => panic!("job ownership unavailable: {reason:?}"),
                ExitProbe::Alive { .. } => {}
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    fn leaf_started(owner: &mut ProcessOwner) {
        let mut output = BufReader::new(owner.take_stdout().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            assert!(output.read_line(&mut line).unwrap() > 0);
            if line.contains("LEAF_STARTED ") {
                break;
            }
        }
        assert!(line.contains("BREAKAWAY_DENIED"));
    }
    #[test]
    fn job_fixture() {
        let Ok(mode) = std::env::var("NB_OWNED_JOB_FIXTURE") else {
            return;
        };
        if mode == "leaf" {
            let mut byte = [0];
            let _ = std::io::stdin().read(&mut byte).unwrap();
            return;
        }
        assert_eq!(mode, "parent");
        let exe = std::env::current_exe().unwrap();
        let blocked = std::process::Command::new(&exe)
            .args([
                "--exact",
                "native_windows_tests::job_fixture",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("NB_OWNED_JOB_FIXTURE", "leaf")
            .creation_flags(CREATE_BREAKAWAY_FROM_JOB.0)
            .spawn();
        assert!(
            blocked.is_err(),
            "fresh owned Job incorrectly permitted breakaway"
        );
        let leaf = std::process::Command::new(exe)
            .args([
                "--exact",
                "native_windows_tests::job_fixture",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("NB_OWNED_JOB_FIXTURE", "leaf")
            .stdin(Stdio::inherit())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        println!("LEAF_STARTED {} BREAKAWAY_DENIED", leaf.id());
        std::io::stdout().flush().unwrap();
        // Job ownership survives the original Child's real exit. Child Drop
        // neither waits nor terminates the leaf; the parent owner must find it.
        drop(leaf);
        std::process::exit(0);
    }
    #[test]
    fn suspended_spawn_job_tracks_descendant_after_root_exit_then_graceful_eof() {
        let dir = profile();
        let lease = ProfileLease::acquire(&dir).unwrap();
        let mut owner = ProcessOwner::spawn_owned(command(), lease).unwrap();
        assert_eq!(owner.hibernation_admission(), Ok(()));
        let root = owner.identity().unwrap();
        assert_eq!(owner.id(), root.pid);
        let input = owner.take_stdin().unwrap();
        leaf_started(&mut owner);
        launcher_exited(&mut owner);
        assert!(matches!(owner.probe(), ExitProbe::Alive { .. }));
        assert!(ProfileLease::acquire(&dir).is_err());
        owner = owner.complete().unwrap_err();
        assert!(ProfileLease::acquire(&dir).is_err());
        drop(input);
        exited(&mut owner);
        owner.complete().unwrap();
        drop(ProfileLease::acquire(&dir).unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn explicit_user_termination_keeps_lease_until_entire_job_exit() {
        let dir = profile();
        let lease = ProfileLease::acquire(&dir).unwrap();
        let mut owner = ProcessOwner::spawn_owned(command(), lease).unwrap();
        assert_eq!(owner.hibernation_admission(), Ok(()));
        let input = owner.take_stdin().unwrap();
        leaf_started(&mut owner);
        launcher_exited(&mut owner);
        assert!(matches!(owner.probe(), ExitProbe::Alive { .. }));
        assert!(ProfileLease::acquire(&dir).is_err());
        match owner.terminate_explicit() {
            Ok(()) => {}
            Err(mut retained) => {
                assert!(ProfileLease::acquire(&dir).is_err());
                exited(&mut retained);
                retained.complete().unwrap();
            }
        }
        drop(input);
        drop(ProfileLease::acquire(&dir).unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
