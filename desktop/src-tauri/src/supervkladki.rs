//! Сохранённые профили и порядок супер-вкладок.
use crate::*;
pub static SOHRANENNYE: Mutex<()> = Mutex::new(());
pub fn obnovit_sohranennye(f: impl FnOnce(&mut Vec<Nastrojki>)) -> Result<(), String> {
    let _lock = SOHRANENNYE.lock().map_err(|e| e.to_string())?;
    let mut list = zagruzit_sohranennye(); f(&mut list); sohranit_sohranennye(&list)
}
#[tauri::command]
pub async fn sohranit_vkladku(app: tauri::AppHandle, s: State<'_, Sostoyanie>, id: String) -> Result<Otvet, String> {
    let n = s.vkladki.lock().map_err(|e| e.to_string())?.get(&id).map(|v| v.nastrojki.clone())
        .ok_or_else(|| "Такой супер-вкладки нет".to_string())?;
    tauri::async_runtime::spawn_blocking(move || obnovit_sohranennye(|v| { v.retain(|v| v.id != n.id); v.push(n); }))
        .await.map_err(|e| e.to_string())??;
    let _ = app.emit("vkladki-izmenilis", &id);
    Ok(da())
}
#[tauri::command]
pub async fn peremestit_super(app: tauri::AppHandle, s: State<'_, Sostoyanie>, id: String, pered: Option<String>) -> Result<Otvet, String> {
    let list = {
        let mut map = s.vkladki.lock().map_err(|e| e.to_string())?;
        let mut order: Vec<_> = map.values().map(|v| (v.nastrojki.poryadok, v.nastrojki.id.clone())).collect();
        order.sort();
        let mut order: Vec<_> = order.into_iter().map(|(_, id)| id).collect();
        let Some(from) = order.iter().position(|n| n == &id) else { return Ok(net("Такой супер-вкладки нет")) };
        if pered.as_deref() == Some(&id) { return Ok(da()) }
        order.remove(from);
        let to = pered.and_then(|id| order.iter().position(|n| n == &id)).unwrap_or(order.len());
        order.insert(to, id);
        for (i, id) in order.iter().enumerate() { map.get_mut(id).unwrap().nastrojki.poryadok = i as i64; }
        map.values().map(|v| v.nastrojki.clone()).collect::<Vec<_>>()
    };
    tauri::async_runtime::spawn_blocking(move || { sohranit_spisok(&list); }).await.map_err(|e| e.to_string())?;
    let _ = app.emit("vkladki-izmenilis", ""); Ok(da())
}
