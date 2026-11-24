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
