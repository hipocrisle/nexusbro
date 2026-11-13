import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import Icon from "./Icons";
export type Vyhod = { id: string; nazvanie: string; adres: string };
export type NastrojkiApp = {
  tema: string; shrift: string; razmer_shrifta: number; plotnost: string; masshtab_stranic: number; vyhody: Vyhod[];
  panel_shirina: number; panel_svernuta: boolean; novaya_stranica: string; poisk: string; avtoobnovleniya: boolean; apparatnoe_uskorenie: boolean;
};
export const poUmolchaniyu: NastrojkiApp = {
  tema: "temnaya", shrift: "system", razmer_shrifta: 14, plotnost: "udobno", masshtab_stranic: 100, vyhody: [],
  panel_shirina: 180, panel_svernuta: false, novaya_stranica: "about:blank", poisk: "duckduckgo", avtoobnovleniya: true, apparatnoe_uskorenie: false,
};
export const shrifty: Record<string, string> = { system: 'system-ui, "Segoe UI", sans-serif', segoe: '"Segoe UI", system-ui, sans-serif', arial: 'Arial, sans-serif', verdana: 'Verdana, sans-serif', georgia: 'Georgia, serif', consolas: 'Consolas, "Liberation Mono", monospace' };
const temy = [{ id: "temnaya", name: "Полночь", color: "#4f9df8" }, { id: "svetlaya", name: "Светлая", color: "#ecf0f7" }, { id: "grafit", name: "Графит", color: "#8593a8" }, { id: "nexus", name: "Nexus", color: "#53d6a0" }, { id: "okean", name: "Океан", color: "#50bfe8" }, { id: "sliva", name: "Слива", color: "#bc91ec" }, { id: "pesok", name: "Песок", color: "#e5c291" }, { id: "sistemnaya", name: "Как в системе", color: "linear-gradient(135deg,#263040 50%,#ecf0f7 50%)" }];
type Section = "vid" | "vkladki" | "set" | "privacy" | "about";
export default function Nastrojki({ value, onSave, onClose }: { value: NastrojkiApp; onSave: (n: NastrojkiApp) => Promise<void>; onClose: () => void }) {
  const [n, setN] = useState({ ...poUmolchaniyu, ...value });
  const [section, setSection] = useState<Section>("vid");
  const [error, setError] = useState(""); const [busy, setBusy] = useState(false); const [version, setVersion] = useState("");
  const [name, setName] = useState(""); const [address, setAddress] = useState(""); const [test, setTest] = useState(""); const [testing, setTesting] = useState(false);
  useEffect(() => { void getVersion().then(setVersion).catch(() => {}); }, []);
  const field = <K extends keyof NastrojkiApp>(key: K, val: NastrojkiApp[K]) => setN((n) => ({ ...n, [key]: val }));
  const sectionNames: [Section, string, string][] = [["vid", "text", "Внешний вид"], ["vkladki", "tabs", "Вкладки и поиск"], ["set", "network", "Сеть и VPN"], ["privacy", "shield", "Приватность"], ["about", "globe", "Обновления и версия"]];
  return <div className="zatemnenie" onKeyDown={(e) => { if (e.key === "Escape" && !busy) onClose(); }}>
    <form className="dialog nastrojki-okno" aria-label="Настройки NexusBro" onSubmit={async (e) => {
      e.preventDefault(); setBusy(true); setError(""); try { await onSave(n); onClose(); } catch (err) { setError(String(err)); } finally { setBusy(false); }
    }}>
      <header className="settings-header"><div><span className="eyebrow">ПОД ВАШ ПРИВЫЧНЫЙ РИТМ</span><h2>Настройки браузера</h2></div><button type="button" className="icon-button" aria-label="Закрыть настройки" disabled={busy} onClick={onClose}><Icon name="close" /></button></header>
      <div className="settings-layout"><nav className="settings-nav" aria-label="Разделы настроек">
        {sectionNames.map(([key, icon, label]) => <button key={key} type="button" className={section === key ? "selected" : ""} aria-current={section === key ? "page" : undefined} onClick={() => { setSection(key); setError(""); }}><Icon name={icon} />{label}</button>)}
        <small className="settings-version">NexusBro {version || "…"}</small>
      </nav><div className="settings-content">
        {section === "vid" && <><h3>Ваше рабочее пространство</h3><p className="muted">Тема, шрифт и плотность интерфейса.</p>
          <div className="theme-grid">{temy.map((t) => <button type="button" key={t.id} className={"theme-option" + (n.tema === t.id ? " selected" : "")} aria-pressed={n.tema === t.id} onClick={() => field("tema", t.id)}><span className="theme-swatch" style={{ background: t.color }} /><span>{t.name}</span>{n.tema === t.id && <Icon name="check" size={14} />}</button>)}</div>
          <label>Шрифт интерфейса<select aria-label="Шрифт интерфейса" value={n.shrift} onChange={(e) => field("shrift", e.target.value)}>{Object.keys(shrifty).map((id) => <option key={id} value={id}>{({ system: "Системный", segoe: "Segoe UI", arial: "Arial", verdana: "Verdana", georgia: "Georgia", consolas: "Consolas" } as Record<string, string>)[id]}</option>)}</select><small>Если шрифт не установлен, используется системная замена.</small></label>
          <div className="settings-pair"><label>Размер текста<select aria-label="Размер текста" value={n.razmer_shrifta} onChange={(e) => field("razmer_shrifta", Number(e.target.value))}>{[12,13,14,15,16,18,20].map((v) => <option key={v} value={v}>{v} px</option>)}</select></label>
          <label>Плотность<select aria-label="Плотность интерфейса" value={n.plotnost} onChange={(e) => field("plotnost", e.target.value)}><option value="udobno">Удобная</option><option value="kompaktno">Компактная</option></select></label></div>
          <div className="font-preview" style={{ fontFamily: shrifty[n.shrift], fontSize: n.razmer_shrifta }}>NexusBro — место для ваших вкладок.<small>Аа Бб Вв · 0123456789</small></div>
          <p className="settings-tip">Ширину левой панели меняйте перетаскиванием её границы.</p>
        </>}
        {section === "vkladki" && <><h3>Открывайте привычным способом</h3><label>Поисковая система<select aria-label="Поисковая система" value={n.poisk} onChange={(e) => field("poisk", e.target.value)}><option value="duckduckgo">DuckDuckGo</option><option value="google">Google</option><option value="yandex">Яндекс</option></select></label>
          <label>Страница новой вкладки<input aria-label="Страница новой вкладки" value={n.novaya_stranica} onChange={(e) => field("novaya_stranica", e.target.value)} placeholder="about:blank" /></label>
          <label>Масштаб страниц<select aria-label="Масштаб страниц" value={n.masshtab_stranic} onChange={(e) => field("masshtab_stranic", Number(e.target.value))}>{[50,67,75,80,90,100,110,125,150,175,200].map((v) => <option key={v} value={v}>{v}%</option>)}</select></label>
          <div className="shortcut-list"><span>Новая вкладка<kbd>Ctrl T</kbd></span><span>Закрыть вкладку<kbd>Ctrl W</kbd></span><span>Переключить вкладку<kbd>Ctrl Tab</kbd></span><span>Адресная строка<kbd>Ctrl L</kbd></span><span>Инструменты разработчика<kbd>F12</kbd></span><span>Исходный код<kbd>Ctrl U</kbd></span></div>
        </>}
        {section === "set" && <><h3>Подключения для супер-вкладок</h3><p className="muted">Сохраните свои подписки и прокси. У каждой супер-вкладки можно выбрать отдельное подключение и узел.</p>
          <div className="connection-list">{n.vyhody.map((v) => <div className="connection-item" key={v.id}><Icon name="network" /><div><strong>{v.nazvanie}</strong><small>{v.adres.startsWith("socks") ? "SOCKS5-прокси" : v.adres.startsWith("vless:") ? "VLESS-узел" : "Подписка / прокси"}</small></div><button type="button" className="icon-button" aria-label={"Удалить подключение «"+v.nazvanie+"»"} onClick={() => field("vyhody", n.vyhody.filter((x) => x.id !== v.id))}><Icon name="close" size={16} /></button></div>)}{n.vyhody.length === 0 && <p className="settings-tip">Пока сохранённых подключений нет.</p>}</div>
          <label>Название подключения<input value={name} onChange={(e) => setName(e.target.value)} placeholder="Мой VPN" /></label><label>Ссылка подписки или адрес прокси<input type="password" autoComplete="off" value={address} onChange={(e) => { setAddress(e.target.value); setTest(""); }} placeholder="https://… / vless://… / socks5://…" /><small>Поддерживаются VLESS-ссылки, подписки со списком ссылок и полные Xray JSON-конфигурации, включая автовыбор Happ.</small></label>
          <div className="connection-actions"><button type="button" disabled={!address.trim() || testing} onClick={async () => { setTesting(true); try { setTest(await invoke<string>("proverit_vyhod", { adres: address })); } catch (e) { setTest(String(e)); } finally { setTesting(false); } }}>{testing ? "Проверяю…" : "Проверить"}</button><button type="button" disabled={!name.trim() || !address.trim()} onClick={() => { field("vyhody", [...n.vyhody, { id: "vyh"+Date.now().toString(36), nazvanie: name.trim(), adres: address.trim() }]); setName(""); setAddress(""); setTest(""); }}>Добавить подключение</button></div>{test && <p className="settings-tip" role="status">{test}</p>}
        </>}
        {section === "privacy" && <><h3>Профили и защита</h3><div className="info-card"><Icon name="shield" size={24} /><div><strong>Своя среда для каждой супер-вкладки</strong><p>Входы на сайты и обычные вкладки разделены по профилям. Подключение к сети выбирается отдельно.</p></div></div><p>Блокировку рекламы включайте в настройках супер-вкладки. Кнопка «Защита» рядом с адресной строкой показывает заблокированные домены и сработавшие правила.</p><p className="settings-tip">Блокировка работает по доменам соединений. Содержимое HTTPS не читается, отдельные рекламные блоки внутри разрешённого домена не различаются.</p>
          <label className="galka"><input type="checkbox" checked={n.apparatnoe_uskorenie} onChange={(e) => field("apparatnoe_uskorenie", e.target.checked)} /><span>Аппаратное ускорение страниц<small>Применится после перезапуска супер-вкладки. При проблемах с изображением отключите.</small></span></label>
        </>}
        {section === "about" && <><div className="about-brand"><Icon name="globe" size={48} /><h3>NexusBro</h3><span className="version-chip">Версия {version || "…"}</span></div><p className="muted">Супер-вкладки для отдельных профилей и подключений. Обычные вкладки — для повседневной работы.</p><label className="galka"><input type="checkbox" checked={n.avtoobnovleniya} onChange={(e) => field("avtoobnovleniya", e.target.checked)} /><span>Проверять обновления при запуске<small>Установка всегда требует подтверждения. Проверить вручную можно кнопкой обновления в левой панели.</small></span></label></>}
      </div></div>
      <footer className="settings-footer">{error && <div role="alert" className="oshibka-nastroek">{error}</div>}<div className="knopki"><button type="button" disabled={busy} onClick={onClose}>Отмена</button><button className="glavnaya" disabled={busy}>{busy ? "Сохраняю…" : "Сохранить"}</button></div></footer>
    </form>
  </div>;
}
