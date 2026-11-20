use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;

pub static ZAPIS: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Obychnaya {
    pub id: u64,
    pub adres: String,
    pub zagolovok: String,
    #[serde(default)]
    pub zakreplena: bool,
    #[serde(default)]
    pub zagruzka: bool,
    #[serde(default)]
    pub otlozhena: bool,
    #[serde(default)]
    pub nazad: bool,
    #[serde(default)]
    pub ikonka: Option<String>,
    #[serde(default)]
    pub vpered: bool,
    #[serde(default)]
    pub privatnaya: bool,
    #[serde(default)]
    pub memory_keep:bool,
    #[serde(default)]
    pub memory_pending:bool,
    /// Local intent only; absent legacy state inherits the Super route.
    #[serde(default)]
    pub route: crate::tab_routes::RouteChoice,
    #[serde(default)]
    pub route_generation: u64,
    /// Runtime state is never restored as an applied route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_error: Option<String>,
    #[serde(default)]
    pub route_pending: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Sessiya {
    pub vkladki: Vec<Obychnaya>,
    pub aktivnaya: u64,
    pub sleduyushchaya: u64,
    #[serde(default)]
    pub zakrytye: Vec<Obychnaya>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "deystvie", rename_all = "snake_case")]
pub enum Komanda {
    OknoNovoe { label:String, parent:u64, x:i32, y:i32, w:u32, h:u32, privatnaya:bool, #[serde(default)] sessiya:Option<Sessiya> },
    OknoKomanda { label:String, komanda:Box<Komanda> },
    OknoZakryt { label:String },
    ObnovitInterfeys,
    GlavnoeSkryto,
    GlavnoePokazat,
    Novaya { adres: String },
    NovayaPrivatnaya { adres:String },
    OpenLink { opener_id:u64, adres:String, privatnaya:bool, background:bool },
    PageContextChoose { token:String, command_id:Option<i32> },
    VosstanovitZakrytuyu,
    ZagruzkaUpravlenie{id:u32,action:String},
    Skachat{adres:String,privatnaya:bool},
    Vybrat { id: u64 },
    Zakryt { id: u64 },
    /// Explicit reload-based unload; never scheduled automatically.
    Usypit { id: u64 },
    MemoryAuto { id:u64 },
    MemorySettings { enabled:bool,minutes:u32 },
    MemoryKeep { id:u64,keep:bool },
    ZakrytDrugie { id: u64, sprava: bool },
    Zakrepit { id: u64, zakreplena: bool },
    Pereyti { id: u64, adres: String },
    Nazad { id: u64 },
    Vpered { id: u64 },
    Obnovit { id: u64 },
    /// F5 / Ctrl+F5 — перезагрузка без кеша.
    ObnovitBezKesha { id: u64 },
    Instrumenty { id: u64 },
    InstrumentyRazmer { procent: u32 },
    Nayti { id: u64, text: String, vpered: bool, dalshe: bool },
    PoiskZakryt { id: u64 },
    Perevesti { id:u64 },
    PerevodOriginal { id:u64 },
    PerevodAvto { id:u64, enabled:bool },
    FonKonteynera { rgb:u32 },
    PasswordForm { id:u64,token:String,origin:String,payload:Option<crate::password_cef::Secret> },
    PasswordsEnabled { enabled:bool },
    BrowserLocked { locked:bool, generation:u64 },
    DannyeStatistika { token:String },
    OchistitDannye { token:String,cookies:bool,cache:bool },
    PerevodOtvet { id:u64, token:String, items:Vec<(u32,String)>, done:u32, total:u32, error:Option<String> },
    Issledovat { id: u64, x: i32, y: i32 },
    Iskhodnik { id: u64 },
    Peremestit { id: u64, pered: Option<u64> },
    Sleduyushchaya { nazad: bool },
    Masshtab { procent: u32 },
}

impl Sessiya {
    pub fn novaya(adres: &str) -> Self {
        let mut s = Self {
            vkladki: Vec::new(),
            aktivnaya: 0,
            sleduyushchaya: 1,
            zakrytye: Vec::new(),
        };
        s.dobavit(adres);
        s
    }

    pub fn dobavit(&mut self, adres: &str) -> u64 {
        let id = self.sleduyushchaya;
        self.sleduyushchaya += 1;
        self.vkladki.push(Obychnaya {
            id,
            adres: adres.into(),
            zagolovok: "Новая вкладка".into(),
            zakreplena: false,
            zagruzka: adres!="about:blank",
            otlozhena: false,
            nazad: false,
            ikonka: None,
            vpered: false,
            privatnaya: false,
            memory_keep:false,
            memory_pending:false,
            route: crate::tab_routes::RouteChoice::Inherit,
            route_generation: 0,
            route_error: None,
            route_pending: false,
        });
        self.aktivnaya = id;
        id
    }

    pub fn peremestit(&mut self, id: u64, pered: Option<u64>) {
        if pered == Some(id) { return }
        let Some(from) = self.vkladki.iter().position(|v| v.id == id) else { return };
        let v = self.vkladki.remove(from);
        let to = pered.and_then(|n| self.vkladki.iter().position(|v| v.id == n)).unwrap_or(self.vkladki.len());
        self.vkladki.insert(to, v);
        self.vkladki.sort_by_key(|v| !v.zakreplena);
    }

    pub fn zagruzit(profil: &Path) -> Self {
        let sbros = || {
            let f = profil.join("obychnye.json");
            let rezerv = profil.join("obychnye.povrezhdena.json");
            if f.exists() && !rezerv.exists() {
                let _ = std::fs::copy(f, rezerv);
            }
            Self::novaya("about:blank")
        };

        let s = std::fs::read(profil.join("obychnye.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Self>(&b).ok());
        let Some(mut s) = s else {
            return sbros();
        };
        let ids: std::collections::HashSet<_> = s.vkladki.iter().map(|v| v.id).collect();
        if s.vkladki.is_empty()
            || ids.len() != s.vkladki.len()
            || ids.iter().any(|id| *id == 0 || *id >= (1_u64 << 53) - 1)
        {
            return sbros();
        }
        if !ids.contains(&s.aktivnaya) {
            s.aktivnaya = s.vkladki[0].id;
        }
        s.sleduyushchaya = ids.iter().max().unwrap() + 1;
        for v in &mut s.vkladki {
            v.zagruzka = false;
            v.otlozhena = true;
            v.memory_pending=false;
            v.route_pending=false;
            v.route_error=None;
            v.route_generation=0;
            v.nazad = false;
            v.vpered = false;
        }
        s.vkladki.sort_by_key(|v| !v.zakreplena);
        s
    }

    pub fn sohranit(&self, profil: &Path) -> anyhow::Result<()> {
        if profil.file_name().and_then(|s|s.to_str()).is_some_and(crate::privatnost::eto_id){return Ok(());}
        let mut public=self.clone();for v in public.vkladki.iter_mut().chain(public.zakrytye.iter_mut()) {
            v.memory_pending=false;
            v.route_pending=false;
            v.route_error=None;
            v.route_generation=0;
        }public.vkladki.retain(|v|!v.privatnaya);public.zakrytye.retain(|v|!v.privatnaya);
        if !public.vkladki.iter().any(|v|v.id==public.aktivnaya){public.aktivnaya=public.vkladki.first().map(|v|v.id).unwrap_or(0);}
        std::fs::create_dir_all(profil)?;
        let put = profil.join("obychnye.json");
        let vremennyj = profil.join("obychnye.tmp");
        std::fs::write(&vremennyj, serde_json::to_vec_pretty(&public)?)?;
        std::fs::rename(vremennyj, put)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Papka(std::path::PathBuf);
    impl Papka {
        fn novaya() -> Self {
            let p = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target/session-tests")
                .join(format!(
                    "{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Papka {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn vosstanovlenie_starogo_formata_i_schetchika() {
        let p = Papka::novaya();
        std::fs::write(p.0.join("obychnye.json"),r#"{"vkladki":[{"id":11,"adres":"about:blank","zagolovok":"One","zagruzka":true},{"id":22,"adres":"about:blank","zagolovok":"Two","zakreplena":true}],"aktivnaya":999,"sleduyushchaya":0}"#).unwrap();
        let mut s = Sessiya::zagruzit(&p.0);
        assert_eq!(s.aktivnaya, 11);
        assert!(s.vkladki.iter().all(|v| v.route == crate::tab_routes::RouteChoice::Inherit));
        assert_eq!(s.vkladki[0].id, 22);
        assert!(!s.vkladki[1].zagruzka);
        assert_eq!(s.dobavit("about:blank"), 23);
        s.sohranit(&p.0).unwrap();
        let s = Sessiya::zagruzit(&p.0);
        assert_eq!(s.vkladki.len(), 3);
        assert_eq!(s.aktivnaya, 23);
    }

    #[test]
    fn route_roundtrip_preserves_intent_but_not_pending_runtime_state() {
        let p = Papka::novaya();
        let mut s = Sessiya::novaya("https://example.test");
        s.vkladki[0].route = crate::tab_routes::RouteChoice::Connection {
            id: crate::tab_routes::ConnectionId::parse("connection-1").unwrap(),
        };
        s.vkladki[0].route_generation = 7;
        s.vkladki[0].route_pending = true;
        s.vkladki[0].route_error = Some("Temporary failure".into());
        s.zakrytye.push(s.vkladki[0].clone());
        s.sohranit(&p.0).unwrap();
        let restored = Sessiya::zagruzit(&p.0);
        for tab in restored.vkladki.iter().chain(restored.zakrytye.iter()) {
            assert_eq!(tab.route, s.vkladki[0].route);
            assert_eq!(tab.route_generation, 0);
            assert!(!tab.route_pending);
            assert!(tab.route_error.is_none());
        }
    }

    #[test]
    fn unknown_route_survives_restore_as_blocked_without_resetting_session() {
        let p = Papka::novaya();
        let mut value = serde_json::to_value(Sessiya::novaya("https://example.test")).unwrap();
        value["vkladki"][0]["route"] = serde_json::json!({"kind":"future-route","secret":"not-persisted"});
        std::fs::write(p.0.join("obychnye.json"), serde_json::to_vec(&value).unwrap()).unwrap();
        let s = Sessiya::zagruzit(&p.0);
        assert_eq!(s.vkladki[0].adres, "https://example.test");
        assert!(matches!(s.vkladki[0].route, crate::tab_routes::RouteChoice::Blocked { .. }));
        s.sohranit(&p.0).unwrap();
        let persisted = std::fs::read_to_string(p.0.join("obychnye.json")).unwrap();
        assert!(!persisted.contains("not-persisted"));
        assert!(!persisted.contains("future-route"));
    }

    #[test]
    fn povrezhdennye_id_ne_padayut_i_ne_stirayut_ishodnik() {
        let p = Papka::novaya();
        let json = r#"{"vkladki":[{"id":1,"adres":"about:blank","zagolovok":"One"},{"id":1,"adres":"about:blank","zagolovok":"Two"}],"aktivnaya":1,"sleduyushchaya":0}"#;
        std::fs::write(p.0.join("obychnye.json"), json).unwrap();
        let s = Sessiya::zagruzit(&p.0);
        assert_eq!(s.vkladki.len(), 1);
        assert_eq!(s.vkladki[0].adres, "about:blank");
        s.sohranit(&p.0).unwrap();
        assert_eq!(
            std::fs::read_to_string(p.0.join("obychnye.povrezhdena.json")).unwrap(),
            json
        );
        Sessiya::zagruzit(&p.0).sohranit(&p.0).unwrap();
        assert_eq!(
            std::fs::read_to_string(p.0.join("obychnye.povrezhdena.json")).unwrap(),
            json
        );
    }
}

#[cfg(test)]
mod order_tests {
    use super::*;
    #[test]
    fn perenos_sohranyaet_aktivnuyu_i_gruppu_zakreplennyh() {
        let mut s = Sessiya::novaya("https://a.test");
        let a=s.aktivnaya; let b=s.dobavit("https://b.test"); let c=s.dobavit("https://c.test");
        s.vkladki.iter_mut().find(|v|v.id==a).unwrap().zakreplena=true;
        s.peremestit(c,Some(a));
        assert_eq!(s.vkladki[0].id,a); assert_eq!(s.vkladki[1].id,c); assert_eq!(s.aktivnaya,c);
        s.peremestit(a,None); assert_eq!(s.vkladki[0].id,a);
        s.peremestit(b,Some(c)); assert_eq!(s.vkladki[1].id,b);
    }
}
