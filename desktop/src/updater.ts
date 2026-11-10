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
 *  prinuditelno — кнопкой: игнорируем часовой интервал и сообщаем итог.
 *  tiho — без всяких сообщений при отсутствии обновлений (автопроверка
 *  при старте). Сетевые ошибки всегда глотаем, чтобы не мешать работе. */
export async function proverit_obnovlenie(
  prinuditelno = false,
  tiho = true,
  soobshchit?: (s: string) => void,
  progress?: (polucheno: number, vsego: number) => void
): Promise<void> {
  try {
    let last = 0;
    if (!prinuditelno) {
      last = parseInt(localStorage.getItem(LAST_CHECK_LS) ?? "0", 10);
      if (Date.now() - last < CHECK_INTERVAL_MS) return;
    }

    const update = await check();
    // Метку ставим ТОЛЬКО после удачной проверки: сбой сети не должен
    // откладывать следующую попытку на час (10.11.2026, поймал user).
    if (!prinuditelno) {
      localStorage.setItem(LAST_CHECK_LS, String(Date.now()));
    }
    if (!update) {
      if (!tiho) soobshchit?.("Обновлений нет — стоит последняя версия.");
      return;
    }

    // Диалог рисуется внутри нашего окна, а поверх лежит чужое окно
    // движка — прячем его, иначе вопрос не видно.
    await invoke("pokazat_dvizhki", { vidno: false });
    let soglasen = false;
    try {
      soglasen = await ask(
        `Вышла версия ${update.version}. Обновить сейчас?`,
        { title: "Обновление NexusBro", kind: "info" }
      );
    } finally {
      await invoke("pokazat_dvizhki", { vidno: true });
    }
    if (!soglasen) return;

    // Установщик перезапустит программу, чужие окна за ней не умеют
    // следовать — прячем их заранее.
    await invoke("pokazat_dvizhki", { vidno: false });

    let soobshchenie = "Скачиваю обновление…";
    let vsego = 0;
    let polucheno = 0;
    await update.downloadAndInstall((sobytie) => {
      if (sobytie.event === "Started" && sobytie.data.contentLength) {
        vsego = sobytie.data.contentLength;
      }
      if (sobytie.event === "Progress") {
        polucheno += sobytie.data.chunkLength;
        progress?.(polucheno, vsego);
      }
      if (sobytie.event === "Finished") {
        soobshchenie = "Ставлю обновление…";
        progress?.(vsego || polucheno, vsego || polucheno);
      }
    });
    console.log(soobshchenie);
    await relaunch();
  } catch (e) {
    if (!tiho) soobshchit?.("Проверка обновления не удалась: " + e);
    console.warn("проверка обновления не удалась:", e);
  }
}
