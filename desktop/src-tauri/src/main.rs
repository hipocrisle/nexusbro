// ⛔ Без этой строки на Windows поверх окна выскакивает чёрная консоль.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Подпроцессы движка и процессы супер-вкладок — перезапуски этого же
    // exe: уходим в ветку CEF ДО Tauri (иначе Tauri поднимет второе окно
    // из каждого рендерера).
    if nexusbro_lib::cef_dvizhok::eto_process_cefa() {
        std::process::exit(nexusbro_lib::cef_dvizhok::process_vhod());
    }
    nexusbro_lib::run()
}
