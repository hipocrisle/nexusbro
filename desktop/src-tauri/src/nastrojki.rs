//! Настройки оболочки. Чтение/запись вызываются только из фоновых команд.
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
static ZAPIS: Mutex<()> = Mutex::new(());

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Nastrojki {
    pub tema: String,
    pub panel_shirina: u32,
    pub panel_svernuta: bool,
    pub novaya_stranica: String,
    pub poisk: String,
    pub avtoobnovleniya: bool,
    pub apparatnoe_uskorenie: bool,
}
impl Default for Nastrojki {
    fn default() -> Self {
        Self {
            tema: "temnaya".into(),
            panel_shirina: 180,
            panel_svernuta: false,
            novaya_stranica: "about:blank".into(),
            poisk: "duckduckgo".into(),
            avtoobnovleniya: true,
            apparatnoe_uskorenie: false,
        }
    }
}
fn fajl() -> std::path::PathBuf {
    crate::brauzer::papka_profilej()
        .parent()
        .unwrap()
        .join("nastrojki.json")
}
pub fn zagruzit() -> Nastrojki {
    std::fs::read(fajl())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}
pub fn sohranit(mut n: Nastrojki) -> Result<Nastrojki, String> {
    if !["temnaya", "svetlaya", "sistemnaya"].contains(&n.tema.as_str()) {
        return Err("Неизвестная тема".into());
    }
    if !["duckduckgo", "google", "yandex"].contains(&n.poisk.as_str()) {
        return Err("Неизвестный поиск".into());
    }
    n.panel_shirina = n.panel_shirina.clamp(140, 400);
    let a = n.novaya_stranica.trim();
    if !(a == "about:blank" || a.starts_with("https://") || a.starts_with("http://")) {
        return Err("Для новой вкладки укажите http://, https:// или about:blank".into());
    }
    n.novaya_stranica = a.into();
    let _zapis = ZAPIS.lock().map_err(|e| e.to_string())?;
    let p = fajl();
    std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
    let tmp = p.with_extension("tmp");
    std::fs::write(
        &tmp,
        serde_json::to_vec_pretty(&n).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    std::fs::rename(tmp, p).map_err(|e| e.to_string())?;
    Ok(n)
}

#[tauri::command]
pub async fn nastrojki_poluchit() -> Result<Nastrojki, String> {
    tauri::async_runtime::spawn_blocking(zagruzit)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn nastrojki_sohranit(nastrojki: Nastrojki) -> Result<Nastrojki, String> {
    tauri::async_runtime::spawn_blocking(move || sohranit(nastrojki))
        .await
        .map_err(|e| e.to_string())?
}
