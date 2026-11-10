// Автообновление: проверка при запуске, скачивание и установка,
// перезапуск на новую версию. Паттерн — как у NexusSSH, только проще:
// один стабильный канал, без выбора beta.

import { check } from "@tauri-apps/plugin-updater";
import { ask } from "@tauri-apps/plugin-dialog";
import { relaunch } from "@tauri-apps/plugin-process";
import { invoke } from "@tauri-apps/api/core";

const LAST_CHECK_LS = "nexusbro.lastUpdateCheck";
// Не дёргаем сеть чаще раза в час — и при старте проверяем не каждый раз.
const CHECK_INTERVAL_MS = 60 * 60 * 1000;

/** Проверить обновление и, если есть, поставить его с перезапуском.
 *  Тихо ничего не делает при сетевых ошибках и если проверка не пора. */
export async function proverit_obnovlenie(): Promise<void> {
  try {
    const last = parseInt(localStorage.getItem(LAST_CHECK_LS) ?? "0", 10);
    if (Date.now() - last < CHECK_INTERVAL_MS) return;
    localStorage.setItem(LAST_CHECK_LS, String(Date.now()));

    const update = await check();
    if (!update) return;

    const soglasen = await ask(
      `Вышла версия ${update.version}. Обновить сейчас?`,
      { title: "Обновление NexusBro", kind: "info" }
    );
    if (!soglasen) return;

    // Движки прячем: установщик перезапустит программу, чужие окна
    // за ней не умеют следовать.
    await invoke("pokazat_dvizhki", { vidno: false });

    let soobshchenie = "Скачиваю обновление…";
    await update.downloadAndInstall((sobytie) => {
      if (sobytie.event === "Finished") soobshchenie = "Ставлю обновление…";
    });
    console.log(soobshchenie);
    await relaunch();
  } catch (e) {
    // Сеть недоступна, GitHub лежит — не мешаем работе.
    console.warn("проверка обновления не удалась:", e);
  }
}
