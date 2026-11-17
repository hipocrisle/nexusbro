fn main() {
    // ⛔ Дочерние процессы движка перезапускают наш exe и должны найти
    // libcef.so рядом с ним без LD_LIBRARY_PATH (Linux). $ORIGIN в rpath
    // решает это для установленной копии и для target/*/release.
    #[cfg(target_os = "linux")]
    {
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
        // SQLite импортера не должен подменять sqlite3_* у CEF/WebKit/NSS.
        // Иначе ELF interposition приводил к SIGSEGV в процессе CEF.
        let map=std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("hide-sqlite.map");
        std::fs::write(&map,"{ local: sqlite3_*; };\n").expect("SQLite visibility map");
        println!("cargo:rustc-link-arg-bin=nexusbro=-Wl,--version-script={}",map.display());
    }
    tauri_build::build()
}
