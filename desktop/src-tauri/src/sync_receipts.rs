//! A portable record is confirmed only by a matching authenticated server receipt.
//! Local snapshot metadata and account-wide success dates are not acknowledgements.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Receipt {
    pub hash: String,
    pub confirmed_at: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordState {
    Off,
    Pending,
    Ok,
    Error,
    Conflict,
}

/// `receipt` must come from the separate persisted server-acknowledgement map.
/// Never synthesize it from a locally captured snapshot or a global sync date.
/// An interrupted/failed exchange invalidates a previous green presentation,
/// while opting out always takes precedence over a former error or conflict.
pub fn classify_record(
    selected: bool,
    current_hash: Option<&str>,
    receipt: Option<&Receipt>,
    conflict: bool,
    exchange_bad: bool,
) -> RecordState {
    if !selected {
        return RecordState::Off;
    }
    if conflict {
        return RecordState::Conflict;
    }
    if exchange_bad {
        return RecordState::Error;
    }
    match (current_hash, receipt) {
        (Some(current), Some(ack)) if !current.is_empty() && current == ack.hash => RecordState::Ok,
        _ => RecordState::Pending,
    }
}

/// Invalidate an old acknowledgement when an authenticated remote record differs.
/// Call before opt-in filtering, so observing changes while disabled cannot leave
/// stale green state after opting in again. `None` means an authenticated tombstone,
/// never an unverified server deletion flag. This changes receipt metadata only.
pub fn invalidate_from_remote(
    receipts: &mut BTreeMap<String, Receipt>,
    record_id: &str,
    remote_hash: Option<&str>,
) {
    let invalidate = receipts
        .get(record_id)
        .is_some_and(|receipt| remote_hash != Some(receipt.hash.as_str()));
    if invalidate {
        receipts.remove(record_id);
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_record, invalidate_from_remote, Receipt, RecordState};
    use serde::{Deserialize, Serialize};
    use std::collections::BTreeMap;

    #[derive(Default, Serialize, Deserialize)]
    #[serde(default)]
    struct PersistedFixture {
        // This has the shape of the old Local.meta and is deliberately separate.
        meta: BTreeMap<String, SnapshotFixture>,
        receipts: BTreeMap<String, Receipt>,
        last_sync: Option<u64>,
        last_exchange_failed: bool,
    }
    #[derive(Default, Serialize, Deserialize)]
    struct SnapshotFixture {
        hash: String,
        modified: u64,
    }

    #[test]
    fn legacy_local_snapshot_and_success_date_do_not_become_an_ack() {
        let mut local: PersistedFixture = serde_json::from_str(
            r#"{"meta":{"super-id":{"hash":"before-edit","modified":12}},"last_sync":20}"#,
        )
        .unwrap();
        let snapshot = local.meta.get_mut("super-id").unwrap();
        snapshot.hash = "new-local-edit".into();
        snapshot.modified = 21;
        assert!(local.receipts.is_empty());
        assert_eq!(
            classify_record(
                true,
                Some("new-local-edit"),
                local.receipts.get("super-id"),
                false,
                false
            ),
            RecordState::Pending
        );
        // Capturing a new snapshot before a failed send cannot manufacture green.
        local.last_exchange_failed = true;
        let restarted: PersistedFixture =
            serde_json::from_str(&serde_json::to_string(&local).unwrap()).unwrap();
        assert_eq!(restarted.meta["super-id"].hash, "new-local-edit");
        assert_eq!(
            classify_record(
                true,
                Some("new-local-edit"),
                restarted.receipts.get("super-id"),
                false,
                restarted.last_exchange_failed
            ),
            RecordState::Error
        );
    }

    #[test]
    fn edit_after_ack_stays_pending_until_that_exact_edit_is_acknowledged() {
        let mut ack = Receipt {
            hash: "portable-v1".into(),
            confirmed_at: 100,
        };
        assert_eq!(
            classify_record(true, Some("portable-v1"), Some(&ack), false, false),
            RecordState::Ok
        );
        assert_eq!(
            classify_record(true, Some("portable-v2"), Some(&ack), false, false),
            RecordState::Pending
        );
        // An accepted echo of an older snapshot still does not confirm v2.
        ack.confirmed_at = 200;
        assert_eq!(
            classify_record(true, Some("portable-v2"), Some(&ack), false, false),
            RecordState::Pending
        );
        ack.hash = "portable-v2".into();
        ack.confirmed_at = 201;
        assert_eq!(
            classify_record(true, Some("portable-v2"), Some(&ack), false, false),
            RecordState::Ok
        );
    }

    #[test]
    fn missing_current_or_receipt_never_confirms_a_record() {
        let ack = Receipt {
            hash: "known".into(),
            confirmed_at: 123,
        };
        assert_eq!(
            classify_record(true, Some("known"), None, false, false),
            RecordState::Pending
        );
        assert_eq!(
            classify_record(true, None, Some(&ack), false, false),
            RecordState::Pending
        );
        assert_eq!(
            classify_record(true, None, None, false, false),
            RecordState::Pending
        );
        assert_eq!(
            classify_record(true, Some(""), Some(&Receipt::default()), false, false),
            RecordState::Pending
        );
    }

    #[test]
    fn opt_out_is_local_even_after_a_failed_or_conflicted_exchange() {
        let ack = Receipt {
            hash: "known".into(),
            confirmed_at: 123,
        };
        assert_eq!(
            classify_record(false, Some("known"), Some(&ack), true, true),
            RecordState::Off
        );
        assert_eq!(
            classify_record(false, None, None, false, true),
            RecordState::Off
        );
    }

    #[test]
    fn conflict_is_specific_and_a_failed_exchange_suppresses_old_green() {
        let ack = Receipt {
            hash: "known".into(),
            confirmed_at: 123,
        };
        assert_eq!(
            classify_record(true, Some("known"), Some(&ack), true, true),
            RecordState::Conflict
        );
        assert_eq!(
            classify_record(true, Some("known"), Some(&ack), false, true),
            RecordState::Error
        );
        assert_eq!(
            classify_record(true, None, None, false, true),
            RecordState::Error
        );
        assert_eq!(
            classify_record(true, Some("known"), Some(&ack), false, false),
            RecordState::Ok
        );
    }

    #[test]
    fn receipt_defaults_and_new_maps_deserialize_without_migrating_snapshot_hashes() {
        let empty: Receipt = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, Receipt::default());
        let older: Receipt = serde_json::from_str(r#"{"hash":"ack-hash"}"#).unwrap();
        assert_eq!(older.hash, "ack-hash");
        assert_eq!(older.confirmed_at, 0);
        let local: PersistedFixture = serde_json::from_str(
            r#"{"meta":{"one":{"hash":"same","modified":1}},"receipts":{"two":{"hash":"actual-ack","confirmed_at":77}}}"#,
        ).unwrap();
        assert!(!local.receipts.contains_key("one"));
        assert_eq!(local.receipts["two"].confirmed_at, 77);
        assert_eq!(
            classify_record(true, Some("same"), local.receipts.get("one"), false, false),
            RecordState::Pending
        );
        assert_eq!(
            classify_record(
                true,
                Some("actual-ack"),
                local.receipts.get("two"),
                false,
                false
            ),
            RecordState::Ok
        );
    }

    #[test]
    fn remote_edit_observed_while_off_requires_new_ack_after_reenabling() {
        let id = "nexusbro.v1/super/one";
        let old_ack = Receipt {
            hash: "portable-v1".into(),
            confirmed_at: 100,
        };
        let mut receipts = BTreeMap::from([(id.into(), old_ack)]);
        assert_eq!(
            classify_record(false, Some("portable-v1"), receipts.get(id), false, false),
            RecordState::Off
        );
        invalidate_from_remote(&mut receipts, id, Some("portable-v2"));
        assert!(!receipts.contains_key(id));
        // Persisting/restarting must not revive the ACK that belonged to v1.
        let restarted: BTreeMap<String, Receipt> =
            serde_json::from_str(&serde_json::to_string(&receipts).unwrap()).unwrap();
        assert_eq!(
            classify_record(true, Some("portable-v1"), restarted.get(id), false, false),
            RecordState::Pending
        );
    }

    #[test]
    fn authenticated_remote_deletion_removes_only_receipt_not_local_metadata() {
        let id = "nexusbro.v1/super/one";
        let other = "nexusbro.v1/super/two";
        let mut local = PersistedFixture::default();
        local.meta.insert(
            id.into(),
            SnapshotFixture {
                hash: "keep-local".into(),
                modified: 21,
            },
        );
        local.receipts.insert(
            id.into(),
            Receipt {
                hash: "keep-local".into(),
                confirmed_at: 100,
            },
        );
        let untouched = Receipt {
            hash: "other-local".into(),
            confirmed_at: 99,
        };
        local.receipts.insert(other.into(), untouched.clone());
        invalidate_from_remote(&mut local.receipts, id, None);
        assert_eq!(local.meta[id].hash, "keep-local");
        assert_eq!(local.meta[id].modified, 21);
        assert_eq!(local.receipts.get(other), Some(&untouched));
        assert_eq!(
            classify_record(
                true,
                Some("keep-local"),
                local.receipts.get(id),
                false,
                false
            ),
            RecordState::Pending
        );
    }

    #[test]
    fn equal_remote_keeps_ack_and_missing_remote_receipt_is_a_noop() {
        let id = "nexusbro.v1/super/one";
        let ack = Receipt {
            hash: "same".into(),
            confirmed_at: 100,
        };
        let mut receipts = BTreeMap::from([(id.into(), ack.clone())]);
        invalidate_from_remote(&mut receipts, id, Some("same"));
        assert_eq!(receipts.get(id), Some(&ack));
        invalidate_from_remote(&mut receipts, "unknown", Some("other"));
        invalidate_from_remote(&mut receipts, "unknown", None);
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts.get(id), Some(&ack));
    }

    #[test]
    fn dto_uses_the_stable_lowercase_state_names() {
        for (state, name) in [
            (RecordState::Off, "off"),
            (RecordState::Pending, "pending"),
            (RecordState::Ok, "ok"),
            (RecordState::Error, "error"),
            (RecordState::Conflict, "conflict"),
        ] {
            assert_eq!(serde_json::to_value(state).unwrap(), name);
            assert_eq!(
                serde_json::from_value::<RecordState>(serde_json::json!(name)).unwrap(),
                state
            );
        }
    }
}
