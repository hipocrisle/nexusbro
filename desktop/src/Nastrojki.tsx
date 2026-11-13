import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";

export type NastrojkiApp = {
  tema: string; panel_shirina: number; panel_svernuta: boolean;
  novaya_stranica: string; poisk: string; avtoobnovleniya: boolean; apparatnoe_uskorenie: boolean;
};
export const poUmolchaniyu: NastrojkiApp = {
  tema: "temnaya", panel_shirina: 180, panel_svernuta: false,
  novaya_stranica: "about:blank", poisk: "duckduckgo", avtoobnovleniya: true, apparatnoe_uskorenie: false,
};

export default function Nastrojki({ value, onSave, onClose }: {
  value: NastrojkiApp; onSave: (n: NastrojkiApp) => Promise<void>; onClose: () => void;
}) {
  const [n, setN] = useState(value);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [version, setVersion] = useState("");
  useEffect(() => { void getVersion().then(setVersion).catch(() => {}); }, []);
  const field = <K extends keyof NastrojkiApp>(key: K, val: NastrojkiApp[K]) => setN({ ...n, [key]: val });
  return <div className="zatemnenie" onKeyDown={(e) => { if (e.key === "Escape" && !busy) onClose(); }}>
    <form className="dialog nastrojki-okno" aria-label="Настройки NexusBro" onSubmit={async (e) => {
      e.preventDefault(); setBusy(true); setError("");
      try { await onSave(n); onClose(); } catch (err) { setError(String(err)); } finally { setBusy(false); }
    }}>
      <h2>Настройки NexusBro</h2>
      <label>Тема<select autoFocus value={n.tema} onChange={(e) => field("tema", e.target.value)}>
        <option value="temnaya">Тёмная</option><option value="svetlaya">Светлая</option><option value="sistemnaya">Как в системе</option>
      </select></label>
      <label>Поисковая система<select value={n.poisk} onChange={(e) => field("poisk", e.target.value)}>
        <option value="duckduckgo">DuckDuckGo</option><option value="google">Google</option><option value="yandex">Яндекс</option>
      </select><small>Поиск из адресной строки, если введён текст вместо адреса.</small></label>
      <label>Страница новой вкладки<input value={n.novaya_stranica} onChange={(e) => field("novaya_stranica", e.target.value)} placeholder="about:blank" /></label>
      <label>Ширина левой панели: {n.panel_shirina} px<input type="range" min="140" max="400" value={n.panel_shirina} onChange={(e) => field("panel_shirina", Number(e.target.value))} /></label>
      <label className="galka"><input type="checkbox" checked={n.avtoobnovleniya} onChange={(e) => field("avtoobnovleniya", e.target.checked)} />
        <span>Проверять обновления при запуске<small>Установка всегда требует подтверждения.</small></span></label>
      <label className="galka"><input type="checkbox" checked={n.apparatnoe_uskorenie} onChange={(e) => field("apparatnoe_uskorenie", e.target.checked)} />
        <span>Аппаратное ускорение страниц<small>Применится после перезапуска супер-вкладки. Если изображение пропадает, отключите.</small></span></label>
      <small>NexusBro {version}</small>
      {error && <div role="alert" className="oshibka-nastroek">{error}</div>}
      <div className="knopki"><button type="button" disabled={busy} onClick={onClose}>Отмена</button><button className="glavnaya" disabled={busy}>{busy ? "Сохраняю…" : "Сохранить"}</button></div>
    </form>
  </div>;
}
