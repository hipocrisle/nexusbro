fn main() {
    // ⛔ Дочерние процессы движка перезапускают наш exe и должны найти
    // libcef.so рядом с ним без LD_LIBRARY_PATH (Linux). $ORIGIN в rpath
    // решает это для установленной копии и для target/*/release.
    #[cfg(target_os = "linux")]
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    tauri_build::build()
}
