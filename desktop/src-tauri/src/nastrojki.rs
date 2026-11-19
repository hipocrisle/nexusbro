//! Настройки оболочки. Чтение/запись вызываются только из фоновых команд.
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
static ZAPIS: Mutex<()> = Mutex::new(());

#[derive(Clone, Serialize, Deserialize)]
pub struct Vyhod { pub id: String, pub nazvanie: String, pub adres: String, #[serde(default)] pub proxy_uzel: String }

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Nastrojki {
    pub tema: String,
    pub shrift: String,
    pub razmer_shrifta: u32,
    pub plotnost: String,
    pub masshtab_stranic: u32,
    pub memory_saver:bool,
    pub memory_saver_minutes:u32,
    pub vyhody: Vec<Vyhod>,
    pub panel_shirina: u32,
    pub panel_svernuta: bool,
    pub panel_zakladok: bool,
    pub novaya_stranica: String,
    pub poisk: String,
    pub avtoobnovleniya: bool,
    pub apparatnoe_uskorenie: bool,
    /// Язык, на который переводить страницу по правому клику.
    pub yazyk_perevoda: String,
}
impl Default for Nastrojki {
    fn default() -> Self {
        Self {
            tema: "temnaya".into(),
            shrift: "system".into(), razmer_shrifta: 14, plotnost: "udobno".into(),
            memory_saver:true,memory_saver_minutes:30,
            masshtab_stranic: 100, vyhody: Vec::new(),
            panel_shirina: 180,
            panel_svernuta: false,
            panel_zakladok: false,
            novaya_stranica: "about:blank".into(),
            poisk: "duckduckgo".into(),
            avtoobnovleniya: true,
            apparatnoe_uskorenie: false,
            yazyk_perevoda: "ru".into(),
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
fn proverit(mut n: Nastrojki) -> Result<Nastrojki, String> {
    if !["temnaya", "svetlaya", "sistemnaya", "grafit", "nexus", "okean", "sliva", "pesok"].contains(&n.tema.as_str()) {
        return Err("Неизвестная тема".into());
    }
    if !["duckduckgo", "google", "yandex"].contains(&n.poisk.as_str()) {
        return Err("Неизвестный поиск".into());
    }
    if !["system", "segoe", "arial", "verdana", "georgia", "consolas"].contains(&n.shrift.as_str()) { return Err("Неизвестный шрифт".into()); }
    if !["udobno", "kompaktno"].contains(&n.plotnost.as_str()) { return Err("Неизвестная плотность интерфейса".into()); }
    n.razmer_shrifta = n.razmer_shrifta.clamp(12, 20);
    let yazyk = n.yazyk_perevoda.trim().to_ascii_lowercase();
    if yazyk.is_empty() || yazyk.len() > 12 || !yazyk.chars().all(|c| c.is_ascii_alphabetic() || c == '-') {
        return Err("Неизвестный язык перевода".into());
    }
    n.yazyk_perevoda = yazyk;
    n.masshtab_stranic = n.masshtab_stranic.clamp(50, 200);
    n.memory_saver_minutes=n.memory_saver_minutes.clamp(15,240);
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
    Ok(n)
}
pub fn sohranit(n:Nastrojki)->Result<Nastrojki,String>{let n=proverit(n)?;let _guard=ZAPIS.lock().map_err(|_|"Настройки заняты")?;zapisat(n)}
fn zapisat(n:Nastrojki)->Result<Nastrojki,String>{
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
    let enabled=saved.memory_saver;let minutes=saved.memory_saver_minutes;let scale=saved.masshtab_stranic;
    tauri::async_runtime::spawn_blocking(move||{
        let mut failed=false;
        for q in queues {
            let mut command=crate::sessiya::Komanda::MemorySettings{enabled,minutes};
            let mut applied=false;
            for _ in 0..20 {
                match q.try_send(command){
                    Ok(())=>{applied=true;break;},
                    Err(std::sync::mpsc::TrySendError::Full(returned))=>{command=returned;std::thread::sleep(std::time::Duration::from_millis(25));},
                    Err(std::sync::mpsc::TrySendError::Disconnected(_))=>break,
                }
            }
            failed|=!applied;
            let _=q.try_send(crate::sessiya::Komanda::Masshtab{procent:scale});
        }
        if failed{Err("Настройки сохранены, но один из движков не принял их. Повторите сохранение.".to_owned())}else{Ok(())}
    }).await.map_err(|e|e.to_string())??;
    use tauri::Emitter;let _=app.emit("nastrojki-izmenilis",&saved);
    Ok(saved)
}

/// Strict reads for sync: missing is a new device; corrupt/unreadable is NOT defaults.
pub fn zagruzit_sync()->Result<Option<Nastrojki>,String>{match std::fs::read(fajl()){Ok(raw)=>serde_json::from_slice(&raw).map(Some).map_err(|_|"Повреждены настройки; синхронизация остановлена".into()),Err(e) if e.kind()==std::io::ErrorKind::NotFound=>Ok(None),Err(_)=>Err("Не удалось прочитать настройки".into())}}
pub fn patch_portable(patch:serde_json::Value,expected:Option<String>)->Result<Nastrojki,String>{
 const FIELDS:&[&str]=&["tema","shrift","razmer_shrifta","plotnost","masshtab_stranic","panel_zakladok","novaya_stranica","poisk","yazyk_perevoda"];
 let _guard=ZAPIS.lock().map_err(|_|"Настройки заняты")?;let current=zagruzit_sync()?;
 let portable=current.as_ref().map(|n|{let all=serde_json::to_value(n).unwrap();let fields:serde_json::Map<_,_>=FIELDS.iter().filter_map(|k|all.get(*k).map(|v|((*k).into(),v.clone()))).collect();serde_json::Value::Object(fields).to_string()});
 if portable!=expected{return Err("Настройки изменились во время синхронизации; повторите попытку".into());}
 let mut all=serde_json::to_value(current.unwrap_or_default()).map_err(|_|"Ошибка настроек")?;
 for field in FIELDS{if let Some(value)=patch.get(*field){all[*field]=value.clone();}}
 let n=serde_json::from_value(all).map_err(|_|"Некорректные настройки")?;zapisat(proverit(n)?)
}
