fn main() {
    let component_manifest = std::path::Path::new("binaries/youtube-light/components.json");
    println!("cargo:rerun-if-changed={}",component_manifest.display());
    let manifest = std::fs::read_to_string(component_manifest).unwrap_or_default();
    assert!(manifest.len() <= 128*1024, "YouTube component manifest exceeds limit");
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    std::fs::write(out.join("youtube-component-pins.rs"),format!("pub const SEALED_YOUTUBE_MANIFEST: &str = {:?};\n",manifest)).expect("YouTube component pins");
    // ⛔ Дочерние процессы движка перезапускают наш exe и должны найти
    // libcef.so рядом с ним без LD_LIBRARY_PATH (Linux). $ORIGIN в rpath
    // решает это для установленной копии и для target/*/release.
    #[cfg(target_os = "linux")]
    {
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN:$ORIGIN/../lib/NexusBro");
        // SQLite импортера не должен подменять sqlite3_* у CEF/WebKit/NSS.
        // Иначе ELF interposition приводил к SIGSEGV в процессе CEF.
        let map=std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("hide-sqlite.map");
        std::fs::write(&map,"{ local: sqlite3_*; };\n").expect("SQLite visibility map");
        println!("cargo:rustc-link-arg-bin=nexusbro=-Wl,--version-script={}",map.display());
    }
    tauri_build::build()
}
