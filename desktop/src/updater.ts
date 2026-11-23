// Автообновление: проверка при запуске, скачивание и установка,
// перезапуск на новую версию. Паттерн — как у NexusSSH, только проще:
// один стабильный канал, без выбора beta.

import { check, type Update } from "@tauri-apps/plugin-updater";
import { ask } from "@tauri-apps/plugin-dialog";
import { relaunch } from "@tauri-apps/plugin-process";
import { invoke } from "@tauri-apps/api/core";

const LAST_CHECK_LS = "nexusbro.lastUpdateCheck";
// Не дёргаем сеть чаще раза в час — и при старте проверяем не каждый раз.
const CHECK_INTERVAL_MS = 60 * 60 * 1000;
const CHECK_TIMEOUT_MS = 15_000;
// Пакет включает CEF и большой: медленный канал не обрываем через минуту.
const DOWNLOAD_TIMEOUT_MS = 20 * 60 * 1000;
// Одна операция на оболочку: повторный клик и StrictMode не должны
// открывать второй диалог или запускать второй установщик.
let tekushchaya: Promise<void> | null = null;

/** Проверить обновление и, если есть, поставить его с перезапуском.
 *  prinuditelno — кнопкой: игнорируем часовой интервал и сообщаем итог.
 *  tiho — без всяких сообщений при отсутствии обновлений (автопроверка
 *  при старте). До согласия ошибки тихой проверки не мешают работе;
 *  после согласия ошибки скачивания/установки всегда показываем. */
export function proverit_obnovlenie(
  prinuditelno = false,
  tiho = true,
  soobshchit?: (s: string) => void,
  progress?: (polucheno: number, vsego: number) => void
): Promise<void> {
  if (tekushchaya) return tekushchaya;
  const run = Promise.resolve().then(() => obnovit(prinuditelno, tiho, soobshchit, progress));
  tekushchaya = run.finally(() => { tekushchaya = null; });
  return tekushchaya;
}

async function obnovit(
  prinuditelno: boolean,
  tiho: boolean,
  soobshchit?: (s: string) => void,
  progress?: (polucheno: number, vsego: number) => void
): Promise<void> {
  let update: Update | null = null;
  let soglasen = false;
  try {
    let last = 0;
    if (!prinuditelno) {
      last = parseInt(localStorage.getItem(LAST_CHECK_LS) ?? "0", 10);
      if (Date.now() - last < CHECK_INTERVAL_MS) return;
    }

    update = await check({ timeout: CHECK_TIMEOUT_MS });
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
    try {
      soglasen = await ask(
        `Вышла версия ${update.version}. Обновить сейчас?`,
        { title: "Обновление NexusBro", kind: "info" }
      );
    } finally {
      await invoke("pokazat_dvizhki", { vidno: true });
    }
    if (!soglasen) return;

    // Скачивание не закрывает браузер: сайты остаются видимыми и доступными.
    // Скрывать движки нужно только перед запуском установщика.
    soobshchit?.("Скачиваю обновление…");
    let vsego = 0;
    let polucheno = 0;
    await update.download((sobytie) => {
      if (sobytie.event === "Started" && sobytie.data.contentLength) {
        vsego = sobytie.data.contentLength;
      }
      if (sobytie.event === "Progress") {
        polucheno += sobytie.data.chunkLength;
        progress?.(polucheno, vsego);
      }
      if (sobytie.event === "Finished") {
        progress?.(vsego || polucheno, vsego || polucheno);
      }
    }, { timeout: DOWNLOAD_TIMEOUT_MS });
    soobshchit?.("Ставлю обновление…");
    await invoke("pokazat_dvizhki", { vidno: false });
    try {
      await update.install();
      await relaunch();
    } finally {
      // Ошибка установки/перезапуска не должна оставлять пустое окно.
      await invoke("pokazat_dvizhki", { vidno: true });
    }
  } catch (e) {
    if (!tiho || soglasen) {
      const etapa = soglasen ? "Не удалось установить обновление" : "Проверка обновления не удалась";
      soobshchit?.(etapa + ": " + String(e) + ". Повторить можно кнопкой обновления.");
    }
    console.warn("обновление не удалось:", e);
  } finally {
    // Отказ/ошибка не должны удерживать ресурсы проверки и скачанные байты.
    // После успешной установки часть ресурсов уже освобождает Rust-плагин.
    await update?.close().catch(() => {});
  }
}
