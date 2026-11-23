//! Bounded local recovery records. Persist only inside the encrypted browser vault.
//! This module never sends history over the sync protocol and never applies it automatically.
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const KEY: &str = "sync.super-history.v1";
const MAX_ENTRIES: usize = 100;
const MAX_BYTES: usize = 512 * 1024;
const MAX_RECORD_BYTES: usize = 32 * 1024;

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct History {
    pub entries: Vec<Entry>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub id: String,
    pub scope: String,
    pub super_id: String,
    pub name: String,
    pub reason: String,
    pub created_at: u64,
    // Includes the previous local route. Never return this field through UI APIs.
    pub local_settings: Value,
}
impl History {
    pub fn parse(raw: Option<&str>) -> Result<Self, String> {
        let Some(raw) = raw else {
            return Ok(Self::default());
        };
        if raw.len() > MAX_BYTES {
            return Err("История восстановления слишком велика".into());
        }
        let history: Self = serde_json::from_str(raw)
            .map_err(|_| "Повреждена история восстановления; данные не изменены")?;
        if history.entries.len() > MAX_ENTRIES {
            return Err("Повреждена история восстановления; данные не изменены".into());
        }
        Ok(history)
    }
    pub fn push(&mut self, entry: Entry) -> Result<(), String> {
        if serde_json::to_vec(&entry)
            .map_err(|_| "Ошибка истории восстановления")?
            .len()
            > MAX_RECORD_BYTES
        {
            return Err("Супер-вкладка слишком велика для безопасного восстановления".into());
        }
        // Repeated rejected remote applies must not evict all useful history.
        if self.entries.last().is_some_and(|old| {
            old.scope == entry.scope
                && old.super_id == entry.super_id
                && old.reason == entry.reason
                && old.local_settings == entry.local_settings
        }) {
            return Ok(());
        }
        self.entries.push(entry);
        while self.entries.len() > MAX_ENTRIES || self.encoded()?.len() > MAX_BYTES {
            self.entries.remove(0);
        }
        Ok(())
    }
    pub fn encoded(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|_| "Ошибка истории восстановления".into())
    }
    pub fn get(&self, id: &str, scope: &str) -> Result<&Entry, String> {
        self.entries
            .iter()
            .find(|entry| entry.id == id && entry.scope == scope)
            .ok_or_else(|| "Запись восстановления отсутствует для этого аккаунта".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(n: usize) -> Entry {
        Entry {
            id: n.to_string(),
            scope: "account-a".into(),
            super_id: "super-a".into(),
            name: "A".into(),
            reason: "cloud_delete".into(),
            created_at: n as u64,
            local_settings: serde_json::json!({"id":"super-a","proxy":"socks5://secret-user:secret@localhost:1234","version":n}),
        }
    }
    #[test]
    fn bounded_and_account_scoped_without_automatic_restore() {
        let mut h = History::default();
        for n in 0..140 {
            h.push(entry(n)).unwrap();
        }
        assert_eq!(h.entries.len(), 100);
        assert!(h.get("0", "account-a").is_err());
        assert!(h.get("139", "account-b").is_err());
        assert_eq!(
            h.get("139", "account-a").unwrap().local_settings["version"],
            139
        );
        assert!(History::parse(Some(&h.encoded().unwrap())).is_ok());
    }
    #[test]
    fn retries_deduplicate_and_invalid_history_is_not_reset() {
        let mut h = History::default();
        h.push(entry(1)).unwrap();
        h.push(entry(1)).unwrap();
        assert_eq!(h.entries.len(), 1);
        assert!(History::parse(Some("{broken")).is_err());
        assert!(History::parse(Some("{\"entries\":[],\"unexpected\":true}")).is_err());
    }
    #[test]
    fn oversized_record_rejected_before_existing_history_changes() {
        let mut h = History::default();
        h.push(entry(1)).unwrap();
        let mut too_big = entry(2);
        too_big.local_settings = Value::String("X".repeat(MAX_RECORD_BYTES));
        assert!(h.push(too_big).is_err());
        assert_eq!(h.entries.len(), 1);
    }
    #[test]
    fn byte_limit_evicts_oldest_but_retains_route() {
        let mut h = History::default();
        for n in 0..100 {
            let mut e = entry(n);
            e.local_settings["padding"] = Value::String("X".repeat(20_000));
            h.push(e).unwrap();
        }
        assert!(h.encoded().unwrap().len() <= MAX_BYTES);
        assert!(h.entries.len() < 100);
        assert_eq!(
            h.get("99", "account-a").unwrap().local_settings["proxy"],
            "socks5://secret-user:secret@localhost:1234"
        );
    }
}
