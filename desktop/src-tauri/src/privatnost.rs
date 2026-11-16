//! Приватные супер-вкладки используют только временный служебный каталог.
use std::path::PathBuf;
pub fn eto_id(id:&str)->bool{id.starts_with("pvt-")}
pub fn process_privatnyj()->bool{std::env::args().any(|a|a=="--nb-private-profile")}
pub fn papka(id:&str)->PathBuf{std::env::temp_dir().join(format!("NexusBro-private-{}",std::process::id())).join(id)}
pub fn ochistit(){
    let root=std::env::temp_dir().join(format!("NexusBro-private-{}",std::process::id()));
    let _=std::fs::remove_dir_all(root);
}
pub fn ochistit_posle_sboya(){
    let Ok(entries)=std::fs::read_dir(std::env::temp_dir()) else{return};
    for entry in entries.flatten(){
        let name=entry.file_name();let name=name.to_string_lossy();
        let Some(pid)=name.strip_prefix("NexusBro-private-").and_then(|s|s.parse::<u32>().ok()) else{continue};
        if !crate::okna::sistema::pid_zhiv(pid){let _=std::fs::remove_dir_all(entry.path());}
    }
}
