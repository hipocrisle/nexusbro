//! Настройки оболочки. Чтение/запись вызываются только из фоновых команд.
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
static ZAPIS: Mutex<()> = Mutex::new(());

#[derive(Clone, Serialize, Deserialize)]
pub struct Vyhod { pub id: String, pub nazvanie: String, pub adres: String }

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Nastrojki {
    pub tema: String,
    pub shrift: String,
    pub razmer_shrifta: u32,
    pub plotnost: String,
    pub masshtab_stranic: u32,
    pub vyhody: Vec<Vyhod>,
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
            shrift: "system".into(), razmer_shrifta: 14, plotnost: "udobno".into(),
            masshtab_stranic: 100, vyhody: Vec::new(),
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
    if !["temnaya", "svetlaya", "sistemnaya", "grafit", "nexus", "okean", "sliva", "pesok"].contains(&n.tema.as_str()) {
        return Err("Неизвестная тема".into());
    }
    if !["duckduckgo", "google", "yandex"].contains(&n.poisk.as_str()) {
        return Err("Неизвестный поиск".into());
    }
    if !["system", "segoe", "arial", "verdana", "georgia", "consolas"].contains(&n.shrift.as_str()) { return Err("Неизвестный шрифт".into()); }
    if !["udobno", "kompaktno"].contains(&n.plotnost.as_str()) { return Err("Неизвестная плотность интерфейса".into()); }
    n.razmer_shrifta = n.razmer_shrifta.clamp(12, 20);
    n.masshtab_stranic = n.masshtab_stranic.clamp(50, 200);
    if n.vyhody.len() > 100 { return Err("Можно сохранить до 100 подключений".into()); }
    let mut ids = std::collections::HashSet::new();
    for v in &mut n.vyhody {
        v.nazvanie = v.nazvanie.trim().into(); v.adres = v.adres.trim().into();
        if v.id.is_empty() || !ids.insert(v.id.clone()) || v.nazvanie.is_empty() || v.nazvanie.chars().count() > 80 || v.adres.is_empty() {
            return Err("У подключения должны быть уникальный ID, название и адрес".into());
        }
        if !crate::vpn::eto_podpiska(&v.adres) && crate::proxy::razobrat_vyhod(&v.adres).is_err() { return Err("Неподдерживаемый адрес подключения".into()); }
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
pub async fn nastrojki_sohranit(app: tauri::AppHandle, nastrojki: Nastrojki) -> Result<Nastrojki, String> {
    let saved = tauri::async_runtime::spawn_blocking(move || sohranit(nastrojki))
        .await
        .map_err(|e| e.to_string())??;
    use tauri::Manager;
    let state: tauri::State<crate::Sostoyanie> = app.state();
    let queues: Vec<_> = state.vkladki.lock().map_err(|e| e.to_string())?.values().filter_map(|v| v.kanal.clone()).collect();
    for q in queues { let _ = q.try_send(crate::sessiya::Komanda::Masshtab { procent: saved.masshtab_stranic }); }
    Ok(saved)
}
