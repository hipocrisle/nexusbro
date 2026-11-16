import { ReactNode, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import Icon from "./Icons";
import { createPortal } from "react-dom";

type Vkladka = { id: number; adres: string; zagolovok: string; zakreplena: boolean; zagruzka: boolean; nazad: boolean; vpered: boolean; otlozhena?: boolean; ikonka?: string | null };
type Sessiya = { vkladki: Vkladka[]; aktivnaya: number };
type Komanda = { deystvie: string; id?: number; adres?: string; zakreplena?: boolean; sprava?: boolean; pered?: number | null; procent?: number };

export default function ObychnyeVkladki({ id, nazvanie, children, novayaStranica, poisk, poleKontenta }: { id: string | null; nazvanie: string; children: ReactNode; novayaStranica: string; poisk: string; poleKontenta: React.RefObject<HTMLDivElement> }) {
  const [sessiya, setSessiya] = useState<Sessiya | null>(null);
  const [adres, setAdres] = useState("");
  const [pravka, setPravka] = useState(false);
  const [oshibka, setOshibka] = useState("");
  const [dragOver, setDragOver] = useState<number | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; v: Vkladka } | null>(null);
  const vybratPriPusteni = useRef(false);
  const pole = useRef<HTMLInputElement>(null);
  const lenta = useRef<HTMLDivElement>(null);
  const sessiyaRef = useRef<Sessiya | null>(null);
  // Ширина, которую даём каждой обычной вкладке. Вкладки не должны растить
  // ползунок: считаем из места под вкладки; когда места мало — «тесно».
  // Стартовое значение — 120px, а не 230px: с 230px первый замер ловил
  // «старую» широкую ленту и ползунок оставался.
  const [shirina, setShirina] = useState({ predel: 120, tesno: false });
  // Плотный режим React'у нужен ПОСЛЕ коммита (подписи уже спрятаны), поэтому
  // ещё один ref: эффект срабатывает позже, чем замер в том же кадре.
  const plotnoRef = useRef(false);
  // Плотный режим (без подписей вкладок и «Защиты») — по ширине полосы.
  const [plotno, setPlotno] = useState(false);
  const skolko = sessiya?.vkladki.length ?? 0;
  const zakrepleno = sessiya?.vkladki.filter((v) => v.zakreplena).length ?? 0;
  // Ширина одной вкладки = (место группы − «+» и зазоры) / число вкладок.
  // ⛔ Порядок важен: сначала вписываем подписи вкладок, не помещаются —
  // прячем их (плотный режим), ещё не помещаемся — прячем подпись «Защиты».
  // Только тогда узкие вкладки, и уже потом ползунок. Меряем свободное место
  // по окну: полоса строится по содержимому, от её ширины — петля.
  const izmerit = useCallback(() => {
    const el = lenta.current;
    if (!el) return;
    const spisok = sessiyaRef.current?.vkladki ?? [];
    const zakreplennyh = spisok.filter((v) => v.zakreplena).length;
    const obyknovennyh = spisok.filter((v) => !v.zakreplena).length;
    const polosa = document.querySelector(".polosa") as HTMLElement | null;
    const knopka = document.querySelector(".protection-button") as HTMLElement | null;
    // Место под вкладки = полоса минус её отступы (16), «+» с зазором
    // группы (34), «Защита» и закреплённые вкладки с зазорами.
    // ⛔ Считать от окна нельзя: справа стоят «Защита» (105px в обычном
    // режиме, 46px в плотном) и отступы — без них вкладки уезжали под неё.
    // «Защиту» берём по БОЛЬШЕМУ значению: в плотном режиме у неё ещё
    // подпись до следующего коммита, а запас карман не тянет.
    // ⛔ offsetWidth у «Защиты» в обычном режиме не совпадает с местом,
    // которое она занимает в полосе (внутри неё отступы и зазоры) — берём
    // её внешнюю ширину.
    const zashchita = Math.max(46, Math.ceil(knopka?.getBoundingClientRect().width ?? 105));
    // ⛔ Каждая вкладка шире своего --w: 2px бордюра (border-box не гасит
    // его при content-размерах вложенных частей) + 5px зазора. Считаем
    // место под вкладку уже с этой «надбавкой», иначе лента длиннее
    // расчёта ровно на 6px × число вкладок (10 вкладок = +60px).
    const zazor = Math.max(0, obyknovennyh - 1) * 5;
    const mesto = Math.max(0, (polosa?.clientWidth ?? document.documentElement.clientWidth)
      - 16 - 30 - 4 - zashchita - zakreplennyh * 44 - zazor);
    // ⛔ Никакого «по ширине вкладки»: у вкладки кроме --w есть ещё 1px
    // бордюра и зазор 5px, поэтому при --w = 127 фактическая ширина 133 —
    // лента на 58px длиннее расчёта. Считаем «место под ВКЛАДКУ» уже с
    // бордюром и зазором.
    const na_vkladku = Math.floor(mesto / Math.max(1, obyknovennyh));
    const podpisi = obyknovennyh > 6 || na_vkladku < 120;
    // Ширину ставим вместе с плотным режимом: атрибут на :root применится
    // в этом же коммите, отдельного пересчёта в следующем кадре не нужно.
    // ⛔ Запас «с подписью» (59px) съедает место НАВСЕГДА: после первого
    // замера полоса уже плотная, но вкладки получают на 59px меньше — и
    // дотянуться до края никогда не могут. Поэтому точный размер берём,
    // когда атрибут на :root уже совпал с нужным режимом; до этого держим
    // запас, чтобы «Защита» не прыгнула на вкладки.
    const rezhim = podpisi ? "da" : "";
    const uzhe = document.documentElement.dataset.tesno === rezhim;
    if (!uzhe) {
      document.documentElement.dataset.tesno = rezhim;
      setPlotno(podpisi);
      plotnoRef.current = podpisi;
    }
    // ⛔ Запас 66px нужен ТОЛЬКО пока на экране старая раскладка «Защиты»
    // с подписью, и снимать его нельзя: иначе после переключения в плотный
    // режим вкладки получают лишние 59px и уезжают под «Защиту». Цена
    // запаса — 59px пустоты в плотном режиме, но без него 8 вкладок по
    // 127px не влезали в 10 (и появлялся ползунок).
    const zapas = 59 + 7;
    void uzhe;
    // ⛔ На 20+ вкладках в узком окне вкладкам не хватает даже 36px на
    // каждую (20 × (36+1+5) = 840px при 605px места), и ленте физически
    // некуда их девать — остаётся «влезло сколько влезло», дальше прокрутка
    // внутри ленты. Минимум 36px сохраняем: так читается иконка и крестик.
    const shirina = obyknovennyh === 0 ? 120
      : Math.max(36, Math.min(230, Math.floor((mesto - zapas) / Math.max(1, obyknovennyh) - 0.6)));
    const tesno = obyknovennyh > 3 && shirina < 160;
    setShirina((s) => (s.predel === shirina && s.tesno === tesno ? s : { predel: shirina, tesno }));
  }, []);
  // Меряем после каждого изменения списка и при смене плотного режима:
  // плотный режим меняет полосу (у «Защиты» пропадает подпись), а React
  // не перерисовывает соседей — без второго замера место остаётся старым.
  // ⛔ Наблюдаем за ПОЛОСОЙ, а не за лентой: ширина ленты зависит от того,
  // сколько места заняли вкладки, — вместе с наблюдателем это давало петлю
  // «мерим → сжимаем → мерим» и бесконечный цикл React (#185).
  useEffect(() => {
    const el = lenta.current;
    if (!el) return;
    const k = () => izmerit();
    // Два кадра: первый — плотный режим применился атрибутом на :root,
    // второй — перестроилась полоса (у «Защиты» ушла подпись).
    let kadr = requestAnimationFrame(() => { k(); kadr = requestAnimationFrame(k); });
    window.addEventListener("resize", k);
    return () => { cancelAnimationFrame(kadr); window.removeEventListener("resize", k); };
  }, [izmerit, skolko, zakrepleno, plotno]);
  useLayoutEffect(izmerit, [izmerit, skolko, zakrepleno, plotno]);
  useEffect(() => {
    setPravka(false);
    setOshibka("");
    if (!id) return;
    let zhiv = true;
    let obnovleniya = 0;
    const otpiska = listen<{ id: string; sessiya: Sessiya }>("obychnye-izmenilis", (e) => {
      if (zhiv && e.payload.id === id) { obnovleniya++; setSessiya(e.payload.sessiya); }
    });
    void otpiska.then(() => invoke<Sessiya>("obychnye_spisok", { id })).then((s) => {
      if (zhiv && obnovleniya === 0) setSessiya(s);
    }).catch((e) => { if (zhiv) setOshibka(String(e)); });
    return () => { zhiv = false; void otpiska.then((f) => f()); };
  }, [id]);
  sessiyaRef.current = sessiya;
  const aktivnaya = sessiya?.vkladki.find((v) => v.id === sessiya.aktivnaya);
  useEffect(() => { if (!pravka) setAdres(aktivnaya?.adres ?? ""); }, [aktivnaya?.adres, aktivnaya?.id, pravka]);
  useEffect(() => {
    if (!aktivnaya || aktivnaya.adres !== "about:blank") return;
    let zhiv = true;
    setAdres("about:blank");
    void invoke("fokus_obolochke").then(() => {
      if (zhiv) { pole.current?.focus(); pole.current?.select(); }
    });
    return () => { zhiv = false; };
  }, [id, aktivnaya?.id]);
  async function zakrytMenu(komanda?: Komanda) {
    setMenu(null);
    try { if (komanda) await poslat(komanda); }
    finally { await invoke("pokazat_dvizhki", { vidno: true }); }
  }
  useEffect(() => {
    if (!menu) return;
    const key = (e: KeyboardEvent) => { if (e.key === "Escape") void zakrytMenu(); };
    document.addEventListener("keydown", key);
    return () => document.removeEventListener("keydown", key);
  }, [menu]);
  async function poslat(komanda: Komanda) {
    if (!id) return;
    setOshibka("");
    try { await invoke("obychnaya_komanda", { id, komanda }); }
    catch (e) { setOshibka(String(e)); }
  }
  useEffect(() => {
    const focusAddress = () => { void invoke("fokus_obolochke").then(() => { pole.current?.focus(); pole.current?.select(); }); };
    const unlisten = listen<string>("adres-vvesti", (e) => { if (e.payload === id) focusAddress(); });
    const key = (e: KeyboardEvent) => {
      const k = e.key.toLowerCase();
      const ctrl = e.ctrlKey || e.metaKey;
      // F5 и Ctrl+F5 обязаны работать как в любом браузере, а не только ⟳.
      if (k !== "f12" && k !== "f5" && !(ctrl && ["t", "w", "l", "u", "r", "tab", "i"].includes(k))) return;
      e.preventDefault();
      if (document.querySelector(".zatemnenie")) return;
      if (ctrl && k === "l") { focusAddress(); return; }
      if (!id || !aktivnaya) return;
      if (k === "f5") void poslat({ deystvie: e.shiftKey || ctrl ? "obnovit_bez_kesha" : "obnovit", id: aktivnaya.id });
      else if (k === "f12" || (ctrl && e.shiftKey && k === "i")) void poslat({ deystvie: "instrumenty", id: aktivnaya.id });
      else if (k === "t") void poslat({ deystvie: "novaya", adres: novayaStranica });
      else if (k === "w") void poslat({ deystvie: "zakryt", id: aktivnaya.id });
      else if (k === "u") void poslat({ deystvie: "iskhodnik", id: aktivnaya.id });
      else if (k === "r") void poslat({ deystvie: "obnovit", id: aktivnaya.id });
      else if (k === "tab" && sessiya) {
        const i = sessiya.vkladki.findIndex((v) => v.id === aktivnaya.id), n = sessiya.vkladki.length;
        void poslat({ deystvie: "vybrat", id: sessiya.vkladki[(i + (e.shiftKey ? n - 1 : 1)) % n].id });
      }
    };
    document.addEventListener("keydown", key, true);
    return () => { document.removeEventListener("keydown", key, true); void unlisten.then((f) => f()); };
  }, [id, aktivnaya?.id, skolko, aktivnaya?.adres, novayaStranica]);
  async function finishDrag(before: number | null, e: React.DragEvent) {
    e.preventDefault(); e.stopPropagation(); setDragOver(null);
    const from = Number(e.dataTransfer.getData("application/nexusbro-tab"));
    try { if (from && from !== before) await poslat({ deystvie: "peremestit", id: from, pered: before }); }
    finally { await invoke("pokazat_dvizhki", { vidno: true }); }
  }
  return <>
    <div className="polosa">
      <span className="aktivnaya-imya" title={nazvanie}>{nazvanie}</span>
      {/* ⛔ min-width:0 у самого tab-group: у grid-элемента «min-width:auto»
          превращается в min-content содержимого, и при 20 узких вкладках
          группа распирала полосу (вкладки уезжали за край и под «+»). */}
      <div className="tab-group" style={{ minWidth: 0 }}><div className="obychnye" ref={lenta} role="tablist" aria-label="Обычные вкладки"
        onDragOver={(e) => { if (e.dataTransfer.types.includes("application/nexusbro-tab")) e.preventDefault(); }} onDrop={(e) => void finishDrag(null, e)}>
        {(sessiya?.vkladki ?? []).map((v) => <div key={v.id} role="tab" aria-label={v.zagolovok || "Новая вкладка"} aria-selected={v.id === sessiya?.aktivnaya} tabIndex={0}
          data-id={v.id} draggable style={{ "--w": shirina.predel + "px" } as React.CSSProperties}
          className={"obychnaya" + (v.id === sessiya?.aktivnaya ? " vybrana" : "") + (v.zakreplena ? " zakreplena" : "") + (v.otlozhena ? " otlozhena" : "") + (shirina.tesno && !v.zakreplena ? " tesno" : "") + (dragOver === v.id ? " drop-before" : "")}
          onDragStart={(e) => { e.dataTransfer.setData("application/nexusbro-tab", String(v.id)); e.dataTransfer.effectAllowed = "move"; void invoke("pokazat_dvizhki", { vidno: false }); }}
          onDragOver={(e) => { if (e.dataTransfer.types.includes("application/nexusbro-tab")) { e.preventDefault(); e.stopPropagation(); setDragOver(v.id); } }}
          onDrop={(e) => void finishDrag(v.id, e)} onDragEnd={() => { setDragOver(null); void invoke("pokazat_dvizhki", { vidno: true }); }}
          onMouseDown={(e) => { if (e.button === 1) e.preventDefault(); }}
          onAuxClick={(e) => { if (e.button === 1) { e.preventDefault(); e.stopPropagation(); void poslat({ deystvie: "zakryt", id: v.id }); } }}
          onContextMenu={async (e) => { e.preventDefault(); e.stopPropagation(); const m = { x: Math.max(8, Math.min(e.clientX, window.innerWidth - 260)), y: Math.max(8, Math.min(e.clientY, window.innerHeight - 350)), v }; await invoke("pokazat_dvizhki", { vidno: false }); setMenu(m); }}
          title={(v.otlozhena ? "Спит — нажмите, чтобы загрузить\n" : "") + v.zagolovok + "\n" + v.adres + (v.zakreplena ? "\nЗакреплена" : "")} onClick={() => void poslat({ deystvie: "vybrat", id: v.id })}
          onKeyDown={(e) => { if (e.target !== e.currentTarget) return; if (e.key === "Enter" || e.key === " ") { e.preventDefault(); void poslat({ deystvie: "vybrat", id: v.id }); } }}>
          <span className={"favicon" + (v.zagruzka ? " loading" : "")}>{v.zagruzka ? <span className="spinner" /> : v.ikonka?.startsWith("data:image/png;base64,") ? <img src={v.ikonka} alt="" draggable={false} /> : <Icon name="globe" size={16} />}</span>
          {!v.zakreplena && <span className="obychnaya-imya">{v.zagolovok || "Новая вкладка"}</span>}
          {!v.zakreplena && <button className="zakryt-obychnuyu" title="Закрыть обычную вкладку" aria-label="Закрыть обычную вкладку"
            onClick={(e) => { e.stopPropagation(); void poslat({ deystvie: "zakryt", id: v.id }); }}><Icon name="close" size={13} /></button>}
        </div>)}
      </div><button className="novaya-obychnaya" title="Новая обычная вкладка" aria-label="Новая обычная вкладка" disabled={!id}
        onClick={() => void poslat({ deystvie: "novaya", adres: novayaStranica })}><Icon name="plus" size={18} /></button>
      </div>
      {children}
    </div>
    <form className="navigaciya" onSubmit={(e) => {
      e.preventDefault();
      if (!aktivnaya) return;
      const text = (pole.current?.value ?? adres).trim();
      const search = poisk === "google" ? "https://www.google.com/search?q=" : poisk === "yandex" ? "https://yandex.ru/search/?text=" : "https://duckduckgo.com/?q=";
      const url = !text ? "about:blank" : /^[a-z][a-z0-9+.-]*:/i.test(text) ? text : !/\s/.test(text) && (text.includes(".") || /^localhost(?::|\/|$)/i.test(text)) ? "https://" + text : search + encodeURIComponent(text);
      void poslat({ deystvie: "pereyti", id: aktivnaya.id, adres: url });
      pole.current?.blur();
    }}>
      <button type="button" title="Назад" disabled={!aktivnaya?.nazad} onClick={() => void poslat({ deystvie: "nazad", id: aktivnaya?.id })}><Icon name="back" size={17} /></button>
      <button type="button" title="Вперёд" disabled={!aktivnaya?.vpered} onClick={() => void poslat({ deystvie: "vpered", id: aktivnaya?.id })}><Icon name="forward" size={17} /></button>
      <button type="button" title="Обновить страницу" disabled={!aktivnaya} onClick={() => void poslat({ deystvie: "obnovit", id: aktivnaya?.id })}><Icon name="reload" size={17} /></button>
      <input ref={pole} aria-label="Адрес страницы" placeholder="Введите адрес" disabled={!aktivnaya}
        value={adres} onFocus={(e) => { setPravka(true); e.currentTarget.select(); vybratPriPusteni.current = true; }} onMouseUp={(e) => { if (vybratPriPusteni.current) { e.preventDefault(); vybratPriPusteni.current = false; } }} onBlur={() => setPravka(false)} onChange={(e) => setAdres(e.target.value)} />
      {oshibka && <span className="oshibka-navigacii" role="alert" title={oshibka}>{oshibka}</span>}
    </form>
    {aktivnaya?.otlozhena && poleKontenta.current && createPortal(<div className="pusto"><Icon name="tabs" size={36} /><h1>Вкладка ждёт открытия</h1><p>Страница не загружалась после запуска. Нажмите вкладку или кнопку ниже, когда она понадобится.</p><button onClick={() => void poslat({ deystvie: "vybrat", id: aktivnaya.id })}>Загрузить вкладку</button></div>, poleKontenta.current)}
    {menu && <div className="zatemnenie-prozrachnoe" onClick={() => void zakrytMenu()} onContextMenu={(e) => { e.preventDefault(); void zakrytMenu(); }}>
      <div className="vypadajka menu-obychnyh" role="menu" aria-label="Действия с вкладкой" style={{ left: menu.x, top: menu.y, bottom: "auto" }} onClick={(e) => e.stopPropagation()}>
        <button role="menuitem" onClick={() => void zakrytMenu({ deystvie: "novaya", adres: novayaStranica })}>Новая вкладка</button>
        <button role="menuitem" onClick={() => void zakrytMenu({ deystvie: "obnovit", id: menu.v.id })}>Обновить</button>
        <button role="menuitem" onClick={() => void zakrytMenu({ deystvie: "novaya", adres: menu.v.adres })}>Дублировать</button>
        <button role="menuitem" onClick={() => void zakrytMenu({ deystvie: "zakrepit", id: menu.v.id, zakreplena: !menu.v.zakreplena })}>{menu.v.zakreplena ? "Открепить" : "Закрепить"}</button>
        <button role="menuitem" onClick={() => void zakrytMenu({ deystvie: "iskhodnik", id: menu.v.id })}>Исходный код страницы</button>
        <button role="menuitem" onClick={() => void zakrytMenu({ deystvie: "instrumenty", id: menu.v.id })}>Инструменты разработчика</button>
        <hr />
        <button role="menuitem" onClick={() => void zakrytMenu({ deystvie: "zakryt", id: menu.v.id })}>Закрыть вкладку</button>
        <button role="menuitem" disabled={!sessiya?.vkladki.some((v) => v.id !== menu.v.id && !v.zakreplena)} onClick={() => void zakrytMenu({ deystvie: "zakryt_drugie", id: menu.v.id, sprava: false })}>Закрыть другие вкладки</button>
        <button role="menuitem" disabled={!sessiya?.vkladki.slice(sessiya.vkladki.findIndex((v) => v.id === menu.v.id) + 1).some((v) => !v.zakreplena)} onClick={() => void zakrytMenu({ deystvie: "zakryt_drugie", id: menu.v.id, sprava: true })}>Закрыть вкладки справа</button>
      </div>
    </div>}
  </>;
}
