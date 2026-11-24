#[path = "../../../desktop/src-tauri/src/super_lifecycle.rs"]
pub mod lifecycle;

#[cfg(test)]
mod tests {
    use super::lifecycle::*;
    fn running() -> Machine {
        let mut m = Machine::new();
        assert_eq!(
            m.apply(Event::StartRequested { generation: 0 }),
            vec![Effect::AcquireLease {
                stamp: Stamp {
                    generation: 1,
                    attempt: 0
                }
            }]
        );
        let stamp = m.stamp();
        assert_eq!(
            m.apply(Event::LeaseAcquired { stamp }),
            vec![Effect::Spawn { stamp }]
        );
        assert!(m.apply(Event::StartReady { stamp }).is_empty());
        assert_eq!(m.phase(), Phase::Running);
        m
    }
    fn preparing(m: &mut Machine) -> Stamp {
        let effects = m.apply(Event::PrepareRequested {
            generation: m.stamp().generation,
            eligibility_revision: 11,
            eligible: true,
        });
        let stamp = m.stamp();
        assert_eq!(
            effects,
            vec![Effect::Prepare {
                stamp,
                eligibility_revision: 11
            }]
        );
        stamp
    }
    fn stopping(m: &mut Machine) -> Stamp {
        let stamp = preparing(m);
        assert!(m
            .apply(Event::Prepared {
                stamp,
                eligibility_revision: 11,
                snapshot_valid: true
            })
            .is_empty());
        assert_eq!(
            m.apply(Event::Commit {
                stamp,
                eligibility_revision: 11
            }),
            vec![Effect::Stop {
                stamp,
                kind: StopKind::Hibernate
            }]
        );
        stamp
    }
    fn exit_and_release(m: &mut Machine) -> Vec<Effect> {
        let stamp = m.stamp();
        m.apply(Event::ShutdownAck { stamp });
        assert_eq!(
            m.apply(Event::TreeExited {
                generation: stamp.generation
            }),
            vec![
                Effect::ReleaseRoutes { stamp },
                Effect::ReleaseLease { stamp }
            ]
        );
        assert!(m.profile_reserved());
        assert!(m.lease_confirmed());
        m.apply(Event::LeaseReleased { stamp })
    }
    #[test]
    fn initial_sleep_is_not_hibernation_or_lease() {
        let m = Machine::new();
        assert_eq!(m.phase(), Phase::Sleeping);
        assert!(!m.successful_hibernation());
        assert!(!m.profile_reserved());
        assert!(!m.lease_confirmed());
    }
    #[test]
    fn reservation_never_spawns_without_native_lease() {
        let mut m = Machine::new();
        m.apply(Event::IntentRequested(Intent::Wake));
        assert!(m.profile_reserved());
        assert!(!m.lease_confirmed());
        for _ in 0..100 {
            assert!(m.apply(Event::IntentRequested(Intent::Wake)).is_empty());
        }
        let stamp = m.stamp();
        assert!(m.apply(Event::StartReady { stamp }).is_empty());
        assert_eq!(m.phase(), Phase::Starting);
        assert_eq!(
            m.apply(Event::LeaseAcquired { stamp }),
            vec![Effect::Spawn { stamp }]
        );
        assert!(m.apply(Event::LeaseAcquired { stamp }).is_empty());
    }
    #[test]
    fn failed_lease_is_not_sleep_success_and_no_retry_loop() {
        let mut m = Machine::new();
        m.apply(Event::IntentRequested(Intent::Wake));
        let stamp = m.stamp();
        let effects = m.apply(Event::LeaseFailed { stamp });
        assert_eq!(
            effects,
            vec![Effect::Report {
                stamp,
                reason: Reason::StartFailure
            }]
        );
        assert_eq!(m.phase(), Phase::Sleeping);
        assert!(!m.profile_reserved());
        assert!(!m.successful_hibernation());
    }
    #[test]
    fn commit_requires_prepared_snapshot_and_matching_revision() {
        let mut m = running();
        let stamp = preparing(&mut m);
        assert!(m
            .apply(Event::Commit {
                stamp,
                eligibility_revision: 11
            })
            .is_empty());
        assert!(m
            .apply(Event::Prepared {
                stamp,
                eligibility_revision: 12,
                snapshot_valid: true
            })
            .is_empty());
        assert!(m
            .apply(Event::Commit {
                stamp,
                eligibility_revision: 12
            })
            .is_empty());
        assert_eq!(m.phase(), Phase::Preparing);
    }
    #[test]
    fn invalid_snapshot_aborts_and_requires_native_ack() {
        let mut m = running();
        let stamp = preparing(&mut m);
        let effects = m.apply(Event::Prepared {
            stamp,
            eligibility_revision: 11,
            snapshot_valid: false,
        });
        assert!(effects.contains(&Effect::AbortPrepare {
            stamp,
            eligibility_revision: 11
        }));
        assert_eq!(m.phase(), Phase::Preparing);
        assert!(m.profile_reserved());
        assert!(m
            .apply(Event::Commit {
                stamp,
                eligibility_revision: 11
            })
            .is_empty());
        m.apply(Event::AbortAck {
            stamp,
            eligibility_revision: 11,
        });
        assert_eq!(m.phase(), Phase::Running);
    }
    #[test]
    fn sleep_needs_snapshot_shutdown_tree_exit_and_lease_release() {
        let mut m = running();
        let stamp = stopping(&mut m);
        assert!(m.apply(Event::LeaseReleased { stamp }).is_empty());
        assert!(m
            .apply(Event::TreeExited {
                generation: stamp.generation
            })
            .is_empty());
        assert_eq!(m.phase(), Phase::Stopping);
        assert!(m.profile_reserved());
        assert_eq!(
            m.apply(Event::ShutdownAck { stamp }),
            vec![
                Effect::ReleaseRoutes { stamp },
                Effect::ReleaseLease { stamp }
            ]
        );
        assert!(!m.successful_hibernation());
        assert_eq!(
            m.apply(Event::LeaseReleased { stamp }),
            vec![Effect::PublishSleeping { stamp }]
        );
        assert!(m.successful_hibernation());
        assert_eq!(m.phase(), Phase::Sleeping);
        assert!(!m.profile_reserved());
        assert!(m.apply(Event::LeaseReleased { stamp }).is_empty());
    }
    #[test]
    fn stopping_deadlines_and_rapid_wake_never_release_or_spawn() {
        let mut m = running();
        let stamp = stopping(&mut m);
        for _ in 0..100 {
            assert_eq!(
                m.apply(Event::Deadline { stamp }),
                vec![Effect::Report {
                    stamp,
                    reason: Reason::StopTimeout
                }]
            );
            assert!(m.apply(Event::IntentRequested(Intent::Wake)).is_empty());
            assert!(m.profile_reserved());
            assert_eq!(m.phase(), Phase::Stopping);
        }
        let effects = exit_and_release(&mut m);
        assert_eq!(
            effects,
            vec![
                Effect::PublishSleeping { stamp },
                Effect::AcquireLease {
                    stamp: Stamp {
                        generation: 2,
                        attempt: 0
                    }
                }
            ]
        );
        assert_eq!(m.phase(), Phase::Starting);
        assert!(!m.lease_confirmed());
    }
    #[test]
    fn prepare_deadline_abort_ack_blocks_commit_and_duplicate_abort() {
        let mut m = running();
        let stamp = preparing(&mut m);
        assert!(m
            .apply(Event::Deadline { stamp })
            .contains(&Effect::AbortPrepare {
                stamp,
                eligibility_revision: 11
            }));
        assert!(!m
            .apply(Event::Deadline { stamp })
            .iter()
            .any(|e| matches!(e, Effect::AbortPrepare { .. })));
        m.apply(Event::Prepared {
            stamp,
            eligibility_revision: 11,
            snapshot_valid: true,
        });
        assert!(m
            .apply(Event::Commit {
                stamp,
                eligibility_revision: 11
            })
            .is_empty());
        m.apply(Event::AbortAck {
            stamp,
            eligibility_revision: 12,
        });
        assert_eq!(m.phase(), Phase::Preparing);
        m.apply(Event::AbortAck {
            stamp,
            eligibility_revision: 11,
        });
        assert_eq!(m.phase(), Phase::Running);
    }
    #[test]
    fn changed_eligibility_cannot_be_revalidated_by_old_prepared_ack() {
        let mut m = running();
        let stamp = preparing(&mut m);
        assert_eq!(
            m.apply(Event::EligibilityChanged { revision: 12 }),
            vec![Effect::AbortPrepare {
                stamp,
                eligibility_revision: 11
            }]
        );
        m.apply(Event::Prepared {
            stamp,
            eligibility_revision: 11,
            snapshot_valid: true,
        });
        assert!(m
            .apply(Event::Commit {
                stamp,
                eligibility_revision: 11
            })
            .is_empty());
    }
    #[test]
    fn close_supersedes_wake_during_preparation_and_stopping() {
        let mut m = running();
        let stamp = preparing(&mut m);
        m.apply(Event::IntentRequested(Intent::Wake));
        m.apply(Event::IntentRequested(Intent::Close));
        m.apply(Event::IntentRequested(Intent::Wake));
        assert_eq!(m.pending_intent(), Some(Intent::Close));
        let effects = m.apply(Event::AbortAck {
            stamp,
            eligibility_revision: 11,
        });
        assert_eq!(
            effects,
            vec![Effect::Stop {
                stamp: m.stamp(),
                kind: StopKind::Close
            }]
        );
        m.apply(Event::IntentRequested(Intent::Wake));
        assert_eq!(exit_and_release(&mut m), vec![Effect::Closed]);
        assert!(m.terminal());
        assert!(!m.successful_hibernation());
        assert!(m.apply(Event::IntentRequested(Intent::Wake)).is_empty());
    }
    #[test]
    fn explicit_intent_during_hibernation_never_publishes_sleep_success() {
        for intent in [Intent::Close, Intent::Edit, Intent::Shutdown] {
            let mut m = running();
            stopping(&mut m);
            m.apply(Event::IntentRequested(intent));
            let effects = exit_and_release(&mut m);
            assert!(!effects
                .iter()
                .any(|e| matches!(e, Effect::PublishSleeping { .. })));
            assert!(!m.successful_hibernation());
            match intent {
                Intent::Close => assert_eq!(effects, vec![Effect::Closed]),
                Intent::Shutdown => assert_eq!(effects, vec![Effect::ShutdownComplete]),
                Intent::Edit => {
                    assert_eq!(effects[0], Effect::ApplyEdit);
                    assert!(matches!(effects[1], Effect::AcquireLease { .. }));
                }
                _ => unreachable!(),
            }
        }
    }
    #[test]
    fn lock_during_preparation_requires_abort_and_prohibits_new_prepare() {
        let mut m = running();
        let stamp = preparing(&mut m);
        m.apply(Event::IntentRequested(Intent::Lock));
        m.apply(Event::AbortAck {
            stamp,
            eligibility_revision: 11,
        });
        assert_eq!(m.phase(), Phase::Running);
        assert!(m.locked());
        assert_eq!(
            m.apply(Event::PrepareRequested {
                generation: 1,
                eligibility_revision: 12,
                eligible: true
            }),
            vec![Effect::Report {
                stamp,
                reason: Reason::Ineligible
            }]
        );
    }
    #[test]
    fn lock_after_commit_delays_queued_wake_until_unlocked() {
        let mut m = running();
        let stamp = stopping(&mut m);
        m.apply(Event::IntentRequested(Intent::Wake));
        m.apply(Event::IntentRequested(Intent::Lock));
        assert_eq!(
            exit_and_release(&mut m),
            vec![Effect::PublishSleeping { stamp }]
        );
        assert_eq!(m.phase(), Phase::Sleeping);
        assert!(!m.profile_reserved());
        assert_eq!(
            m.apply(Event::Unlocked),
            vec![Effect::AcquireLease {
                stamp: Stamp {
                    generation: 2,
                    attempt: 0
                }
            }]
        );
    }
    #[test]
    fn close_before_lease_acquired_never_spawns_and_retains_acquired_lease() {
        let mut m = Machine::new();
        m.apply(Event::IntentRequested(Intent::Wake));
        let stamp = m.stamp();
        m.apply(Event::IntentRequested(Intent::Close));
        assert_eq!(
            m.apply(Event::LeaseAcquired { stamp }),
            vec![Effect::Stop {
                stamp: m.stamp(),
                kind: StopKind::Close
            }]
        );
        assert!(m.lease_confirmed());
        assert_eq!(exit_and_release(&mut m), vec![Effect::Closed]);
    }
    #[test]
    fn start_failure_waits_entire_tree_and_never_claims_sleep() {
        let mut m = Machine::new();
        m.apply(Event::IntentRequested(Intent::Wake));
        let stamp = m.stamp();
        m.apply(Event::LeaseAcquired { stamp });
        let effects = m.apply(Event::StartFailed { stamp });
        assert!(effects.contains(&Effect::Stop {
            stamp,
            kind: StopKind::StartFailure
        }));
        assert!(m.profile_reserved());
        assert_eq!(m.phase(), Phase::Stopping);
        assert_eq!(
            m.apply(Event::TreeExited { generation: 1 }),
            vec![
                Effect::ReleaseRoutes { stamp },
                Effect::ReleaseLease { stamp }
            ]
        );
        assert!(m.apply(Event::LeaseReleased { stamp }).is_empty());
        assert!(!m.successful_hibernation());
    }
    #[test]
    fn crash_is_not_hibernation_and_stale_native_events_are_inert() {
        let mut m = running();
        let old = m.stamp();
        m.apply(Event::TreeExited {
            generation: old.generation,
        });
        m.apply(Event::LeaseReleased { stamp: old });
        assert!(!m.successful_hibernation());
        m.apply(Event::IntentRequested(Intent::Wake));
        let next = m.stamp();
        for e in [
            Event::StartReady { stamp: old },
            Event::LeaseAcquired { stamp: old },
            Event::LeaseReleased { stamp: old },
            Event::ShutdownAck { stamp: old },
            Event::TreeExited {
                generation: old.generation,
            },
            Event::Deadline { stamp: old },
        ] {
            assert!(m.apply(e).is_empty());
            assert_eq!(m.stamp(), next);
            assert_eq!(m.phase(), Phase::Starting);
        }
    }
    #[test]
    fn duplicate_prepared_commit_and_exit_do_not_duplicate_side_effects() {
        let mut m = running();
        let stamp = stopping(&mut m);
        for e in [
            Event::Prepared {
                stamp,
                eligibility_revision: 11,
                snapshot_valid: true,
            },
            Event::Commit {
                stamp,
                eligibility_revision: 11,
            },
        ] {
            assert!(m.apply(e).is_empty());
        }
        m.apply(Event::ShutdownAck { stamp });
        m.apply(Event::TreeExited { generation: 1 });
        assert!(m.apply(Event::TreeExited { generation: 1 }).is_empty());
        assert!(m.apply(Event::ShutdownAck { stamp }).is_empty());
    }
    #[test]
    fn shutdown_cannot_be_downgraded_to_close_or_edit() {
        let mut m = running();
        m.apply(Event::IntentRequested(Intent::Shutdown));
        for intent in [Intent::Close, Intent::Edit, Intent::Wake, Intent::Lock] {
            m.apply(Event::IntentRequested(intent));
        }
        assert_eq!(exit_and_release(&mut m), vec![Effect::ShutdownComplete]);
    }
    #[test]
    fn arbitrary_completion_permutations_never_spawn_without_lease() {
        // Exhaust all length-four adversarial prefixes, including acknowledgments
        // before their requests. Effects/ownership invariants hold for each prefix.
        let events = [
            Event::IntentRequested(Intent::Wake),
            Event::IntentRequested(Intent::Lock),
            Event::Unlocked,
            Event::LeaseAcquired {
                stamp: Stamp {
                    generation: 1,
                    attempt: 0,
                },
            },
            Event::StartReady {
                stamp: Stamp {
                    generation: 1,
                    attempt: 0,
                },
            },
            Event::TreeExited { generation: 1 },
            Event::LeaseReleased {
                stamp: Stamp {
                    generation: 1,
                    attempt: 0,
                },
            },
            Event::IntentRequested(Intent::Close),
        ];
        for a in events {
            for b in events {
                for c in events {
                    for d in events {
                        let mut m = Machine::new();
                        let mut spawned = 0;
                        for e in [a, b, c, d] {
                            for effect in m.apply(e) {
                                if let Effect::Spawn { .. } = effect {
                                    spawned += 1;
                                    assert!(m.profile_reserved());
                                    assert!(m.lease_confirmed());
                                    assert_eq!(m.phase(), Phase::Starting);
                                }
                            }
                            assert!(m.phase() == Phase::Sleeping || m.profile_reserved());
                        }
                        assert!(spawned <= 1);
                    }
                }
            }
        }
    }
}
