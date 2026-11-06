// ⛔ Без этой строки на Windows поверх окна выскакивает чёрная консоль.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    nexusbro_lib::run()
}
