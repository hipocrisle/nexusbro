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
    pub nazad: bool,
    #[serde(default)]
    pub ikonka: Option<String>,
    #[serde(default)]
    pub vpered: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Sessiya {
    pub vkladki: Vec<Obychnaya>,
    pub aktivnaya: u64,
    pub sleduyushchaya: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "deystvie", rename_all = "snake_case")]
pub enum Komanda {
    Novaya { adres: String },
    Vybrat { id: u64 },
    Zakryt { id: u64 },
    ZakrytDrugie { id: u64, sprava: bool },
    Zakrepit { id: u64, zakreplena: bool },
    Pereyti { id: u64, adres: String },
    Nazad { id: u64 },
    Vpered { id: u64 },
    Obnovit { id: u64 },
    Instrumenty { id: u64 },
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
            zagruzka: false,
            nazad: false,
            ikonka: None,
            vpered: false,
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
            v.nazad = false;
            v.vpered = false;
        }
        s.vkladki.sort_by_key(|v| !v.zakreplena);
        s
    }

    pub fn sohranit(&self, profil: &Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(profil)?;
        let put = profil.join("obychnye.json");
        let vremennyj = profil.join("obychnye.tmp");
        std::fs::write(&vremennyj, serde_json::to_vec_pretty(self)?)?;
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
        assert_eq!(s.vkladki[0].id, 22);
        assert!(!s.vkladki[1].zagruzka);
        assert_eq!(s.dobavit("about:blank"), 23);
        s.sohranit(&p.0).unwrap();
        let s = Sessiya::zagruzit(&p.0);
        assert_eq!(s.vkladki.len(), 3);
        assert_eq!(s.aktivnaya, 23);
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
