//! Pure ownership and aggregation model for an on-demand process snapshot.
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
pub struct Process {
    pub pid: u32,
    pub parent: u32,
    pub created: u64,
    pub cpu_time_ms: u64,
    pub working_set_bytes: Option<u64>,
    pub private_bytes: Option<u64>,
}
pub struct Root {
    pub pid: u32,
    pub super_id: String,
    pub name: String,
}
/// Separate authority produced ONLY by the native ProcessOwner, never by UI
/// PID input or the legacy Root list. No Deserialize implementation is exposed.
pub struct NativeMember {
    pub pid: u32,
    pub created: u64,
    pub super_id: String,
    pub name: String,
}
#[derive(Serialize)]
pub struct ProcessSample {
    pub pid: u32,
    pub identity: String,
    pub cpu_time_ms: u64,
    pub working_set_bytes: Option<u64>,
    pub private_bytes: Option<u64>,
}
#[derive(Serialize)]
pub struct Group {
    pub id: String,
    pub super_id: Option<String>,
    pub name: String,
    pub processes: usize,
    pub working_set_bytes: Option<u64>,
    pub private_bytes: Option<u64>,
    pub cpu_time_ms: u64,
    pub cpu_samples: Vec<ProcessSample>,
}
#[derive(Serialize)]
pub struct Groups {
    pub groups: Vec<Group>,
    pub totals: Group,
    pub excluded_unverified_roots: usize,
    pub rejected_ancestry: usize,
    pub rejected_native_members: usize,
}
pub fn linux_stat(text: &str, pid: u32, ticks: u64, page: u64) -> Option<Process> {
    if ticks == 0 || page == 0 || text.split_whitespace().next()?.parse::<u32>().ok()? != pid {
        return None;
    }
    // comm can contain spaces and ')' characters. Numeric fields follow its last ')'.
    let (_, tail) = text.rsplit_once(')')?;
    let fields: Vec<_> = tail.split_whitespace().collect();
    let parent = fields.get(1)?.parse().ok()?;
    let user: u64 = fields.get(11)?.parse().ok()?;
    let kernel: u64 = fields.get(12)?.parse().ok()?;
    let created = fields.get(19)?.parse().ok()?;
    let rss: i64 = fields.get(21)?.parse().ok()?;
    Some(Process {
        pid,
        parent,
        created,
        cpu_time_ms: user.saturating_add(kernel).saturating_mul(1000) / ticks,
        working_set_bytes: Some((rss.max(0) as u64).saturating_mul(page)),
        // A zombie has no address space; smaps_rollup is unavailable, not a
        // missing measurement of live memory. Preserve its CPU/creation record.
        private_bytes: matches!(*fields.first()?, "Z" | "X" | "x").then_some(0),
    })
}
pub fn linux_private(text: &str) -> Option<u64> {
    let mut total = 0u64;
    let mut seen = BTreeSet::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if matches!(key, "Private_Clean" | "Private_Dirty" | "Private_Hugetlb") {
            if !seen.insert(key) {
                return None;
            }
            let mut fields = value.split_whitespace();
            let amount: u64 = fields.next()?.parse().ok()?;
            if fields.next() != Some("kB") || fields.next().is_some() {
                return None;
            }
            total = total.saturating_add(amount.saturating_mul(1024));
        }
    }
    (seen.contains("Private_Clean") && seen.contains("Private_Dirty")).then_some(total)
}
fn chain(table: &BTreeMap<u32, Process>, pid: u32, app: u32) -> Option<Vec<u32>> {
    let mut path = Vec::new();
    let mut seen = BTreeSet::new();
    let mut current = pid;
    for _ in 0..128 {
        if !seen.insert(current) {
            return None;
        }
        let node = table.get(&current)?;
        path.push(current);
        if current == app {
            return Some(path);
        }
        let parent = table.get(&node.parent)?;
        // A recycled parent PID must not confer ownership on an older child.
        if parent.created > node.created {
            return None;
        }
        current = parent.pid;
    }
    None
}
fn empty(id: String, super_id: Option<String>, name: String) -> Group {
    Group {
        id,
        super_id,
        name,
        processes: 0,
        working_set_bytes: Some(0),
        private_bytes: Some(0),
        cpu_time_ms: 0,
        cpu_samples: vec![],
    }
}
fn add(group: &mut Group, node: &Process) {
    group.processes += 1;
    group.working_set_bytes = group
        .working_set_bytes
        .zip(node.working_set_bytes)
        .map(|(a, b)| a.saturating_add(b));
    group.private_bytes = group
        .private_bytes
        .zip(node.private_bytes)
        .map(|(a, b)| a.saturating_add(b));
    group.cpu_time_ms = group.cpu_time_ms.saturating_add(node.cpu_time_ms);
    group.cpu_samples.push(ProcessSample {
        pid: node.pid,
        identity: format!("{}:{}", node.pid, node.created),
        cpu_time_ms: node.cpu_time_ms,
        working_set_bytes: node.working_set_bytes,
        private_bytes: node.private_bytes,
    });
}
pub fn aggregate(
    table: &BTreeMap<u32, Process>,
    app: u32,
    roots: &[Root],
) -> Result<Groups, String> {
    aggregate_with_verified(table, app, roots, &[])
}
pub fn aggregate_with_verified(
    table: &BTreeMap<u32, Process>,
    app: u32,
    roots: &[Root],
    native: &[NativeMember],
) -> Result<Groups, String> {
    if !table.contains_key(&app) {
        return Err("Не удалось получить состояние собственного процесса".into());
    }
    let verified: BTreeMap<_, _> = roots
        .iter()
        .filter(|r| chain(table, r.pid, app).is_some())
        .map(|r| (r.pid, r))
        .collect();
    let excluded = roots
        .iter()
        .filter(|r| !verified.contains_key(&r.pid))
        .count();
    let mut native_verified = BTreeMap::<u32, &NativeMember>::new();
    let mut conflicts = BTreeSet::new();
    let mut rejected_native = 0;
    for member in native {
        if member.pid == app
            || member.created == 0
            || !table
                .get(&member.pid)
                .is_some_and(|p| p.created == member.created)
        {
            rejected_native += 1;
            continue;
        }
        if conflicts.contains(&member.pid) {
            rejected_native += 1;
            continue;
        }
        if let Some(previous) = native_verified.get(&member.pid) {
            if previous.created != member.created || previous.super_id != member.super_id {
                native_verified.remove(&member.pid);
                conflicts.insert(member.pid);
                rejected_native += 1;
            }
        } else {
            native_verified.insert(member.pid, member);
        }
    }
    let mut groups = BTreeMap::new();
    let mut totals = empty("all".into(), None, "Всего в этом экземпляре".into());
    let mut rejected = 0;
    for node in table.values() {
        // Native proof grants exactly this PID+birth, not arbitrary descendants
        // claiming it as parent. Other rows still require the old app ancestry.
        let native_owner = native_verified.get(&node.pid);
        let path = chain(table, node.pid, app);
        if native_owner.is_none() && path.is_none() {
            rejected += 1;
            continue;
        }
        let root = path
            .as_ref()
            .and_then(|path| path.iter().find_map(|pid| verified.get(pid)));
        let (id, super_id, name) = if let Some(member) = native_owner {
            (
                format!("super:{}", member.super_id),
                Some(member.super_id.clone()),
                member.name.clone(),
            )
        } else {
            root.map_or_else(
                || ("shell".into(), None, "Оболочка и общие процессы".into()),
                |root| {
                    (
                        format!("super:{}", root.super_id),
                        Some(root.super_id.clone()),
                        root.name.clone(),
                    )
                },
            )
        };
        add(
            groups
                .entry(id.clone())
                .or_insert_with(|| empty(id, super_id, name)),
            node,
        );
        add(&mut totals, node);
    }
    Ok(Groups {
        groups: groups.into_values().collect(),
        totals,
        excluded_unverified_roots: excluded,
        rejected_ancestry: rejected,
        rejected_native_members: rejected_native,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn p(pid: u32, parent: u32, created: u64) -> Process {
        Process {
            pid,
            parent,
            created,
            cpu_time_ms: 10,
            working_set_bytes: Some(100),
            private_bytes: Some(40),
        }
    }
    fn root(pid: u32, id: &str) -> Root {
        Root {
            pid,
            super_id: id.into(),
            name: id.into(),
        }
    }
    #[test]
    fn only_owned_descendants_and_nearest_super_no_double_counting() {
        let table = [
            p(1, 0, 1),
            p(2, 1, 2),
            p(3, 2, 3),
            p(4, 1, 4),
            p(5, 4, 5),
            p(99, 0, 1),
        ]
        .into_iter()
        .map(|p| (p.pid, p))
        .collect();
        let result = aggregate(
            &table,
            1,
            &[
                root(2, "a"),
                root(3, "b"),
                root(4, "c"),
                root(99, "other-instance"),
            ],
        )
        .unwrap();
        assert_eq!(result.totals.processes, 5);
        assert_eq!(result.totals.private_bytes, Some(200));
        assert_eq!(result.excluded_unverified_roots, 1);
        assert_eq!(
            result
                .groups
                .iter()
                .find(|r| r.id == "super:a")
                .unwrap()
                .processes,
            1
        );
        assert_eq!(
            result
                .groups
                .iter()
                .find(|r| r.id == "super:b")
                .unwrap()
                .processes,
            1
        );
        assert_eq!(
            result
                .groups
                .iter()
                .find(|r| r.id == "super:c")
                .unwrap()
                .processes,
            2
        );
    }
    #[test]
    fn pid_reuse_missing_ancestors_and_cycles_fail_closed() {
        let table = [
            p(1, 0, 10),
            p(2, 1, 9),
            p(3, 999, 15),
            p(4, 5, 20),
            p(5, 4, 20),
        ]
        .into_iter()
        .map(|p| (p.pid, p))
        .collect();
        let result = aggregate(&table, 1, &[root(2, "recycled"), root(4, "cycle")]).unwrap();
        assert_eq!(result.totals.processes, 1);
        assert_eq!(result.excluded_unverified_roots, 2);
    }
    #[test]
    fn missing_memory_is_unknown_not_zero_and_identity_changes_on_pid_reuse() {
        let mut child = p(2, 1, 2);
        child.private_bytes = None;
        let table = [p(1, 0, 1), child]
            .into_iter()
            .map(|p| (p.pid, p))
            .collect();
        let result = aggregate(&table, 1, &[]).unwrap();
        assert_eq!(result.totals.private_bytes, None);
        assert_eq!(result.totals.working_set_bytes, Some(200));
        assert_eq!(result.totals.cpu_samples[1].identity, "2:2");
    }
    #[test]
    fn linux_parsers_handle_comm_parentheses_and_real_units_without_shared_pages() {
        let mut fields = vec!["0"; 22];
        fields[0] = "S";
        fields[1] = "1";
        fields[11] = "250";
        fields[12] = "50";
        fields[19] = "123456";
        fields[21] = "7";
        let text = format!("42 (name with ) spaces) {}", fields.join(" "));
        let process = linux_stat(&text, 42, 100, 4096).unwrap();
        assert_eq!(process.parent, 1);
        assert_eq!(process.created, 123456);
        assert_eq!(process.cpu_time_ms, 3000);
        assert_eq!(process.working_set_bytes, Some(28672));
        assert!(linux_stat(&text, 43, 100, 4096).is_none());
        assert!(linux_stat(&text, 42, 0, 4096).is_none());
        assert_eq!(linux_private("Rss: 400 kB\nShared_Clean: 300 kB\nPrivate_Clean: 30 kB\nPrivate_Dirty: 70 kB\nPrivate_Hugetlb: 2 kB\n"),Some(102*1024));
        assert!(linux_private("Private_Clean: 30 kB\n").is_none());
        assert!(linux_private("Private_Clean: 30 kB\nPrivate_Dirty: 70 MB\n").is_none());
        assert!(linux_private(
            "Private_Clean: 30 kB\nPrivate_Clean: 30 kB\nPrivate_Dirty: 70 kB\n"
        )
        .is_none());
    }
    fn native(pid: u32, created: u64, id: &str) -> NativeMember {
        NativeMember {
            pid,
            created,
            super_id: id.into(),
            name: id.into(),
        }
    }
    #[test]
    fn exact_native_orphan_is_included_without_granting_ancestry_authority() {
        let table = [p(1, 0, 1), p(20, 999, 200), p(21, 20, 201), p(99, 0, 1)]
            .into_iter()
            .map(|p| (p.pid, p))
            .collect();
        let plain = aggregate(&table, 1, &[root(20, "forged-root")]).unwrap();
        assert_eq!(plain.totals.processes, 1);
        let verified =
            aggregate_with_verified(&table, 1, &[], &[native(20, 200, "owned")]).unwrap();
        assert_eq!(verified.totals.processes, 2);
        assert_eq!(verified.rejected_ancestry, 2);
        assert_eq!(
            verified
                .groups
                .iter()
                .find(|g| g.id == "super:owned")
                .unwrap()
                .cpu_samples[0]
                .identity,
            "20:200"
        );
        assert!(!verified.totals.cpu_samples.iter().any(|p| p.pid == 21));
    }
    #[test]
    fn recycled_or_missing_native_identity_is_rejected_not_pid_authority() {
        let table = [p(1, 0, 1), p(20, 999, 201)]
            .into_iter()
            .map(|p| (p.pid, p))
            .collect();
        let result = aggregate_with_verified(
            &table,
            1,
            &[],
            &[native(20, 200, "stale"), native(77, 2, "missing")],
        )
        .unwrap();
        assert_eq!(result.totals.processes, 1);
        assert_eq!(result.rejected_native_members, 2);
    }
    #[test]
    fn native_and_legacy_overlap_or_duplicates_count_pid_once() {
        let table = [p(1, 0, 1), p(2, 1, 2), p(3, 2, 3)]
            .into_iter()
            .map(|p| (p.pid, p))
            .collect();
        let result = aggregate_with_verified(
            &table,
            1,
            &[root(2, "owner")],
            &[
                native(2, 2, "owner"),
                native(2, 2, "owner"),
                native(3, 3, "owner"),
            ],
        )
        .unwrap();
        assert_eq!(result.totals.processes, 3);
        assert_eq!(result.totals.cpu_time_ms, 30);
        assert_eq!(result.rejected_native_members, 0);
        assert_eq!(
            result
                .groups
                .iter()
                .find(|g| g.id == "super:owner")
                .unwrap()
                .processes,
            2
        );
    }
    #[test]
    fn conflicting_native_owner_claim_does_not_authorize_foreign_pid() {
        let table = [p(1, 0, 1), p(20, 999, 200)]
            .into_iter()
            .map(|p| (p.pid, p))
            .collect();
        let result = aggregate_with_verified(
            &table,
            1,
            &[],
            &[native(20, 200, "a"), native(20, 200, "b")],
        )
        .unwrap();
        assert_eq!(result.totals.processes, 1);
        assert_eq!(result.rejected_native_members, 1);
    }
}
