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
        private_bytes: None,
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
    let mut groups = BTreeMap::new();
    let mut totals = empty("all".into(), None, "Всего в этом экземпляре".into());
    let mut rejected = 0;
    for node in table.values() {
        let Some(path) = chain(table, node.pid, app) else {
            rejected += 1;
            continue;
        };
        let root = path.iter().find_map(|pid| verified.get(pid));
        let (id, super_id, name) = root.map_or_else(
            || ("shell".into(), None, "Оболочка и общие процессы".into()),
            |root| {
                (
                    format!("super:{}", root.super_id),
                    Some(root.super_id.clone()),
                    root.name.clone(),
                )
            },
        );
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
}
