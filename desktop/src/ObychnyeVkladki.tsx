import BookmarkBar from "./BookmarkBar";
import AddressMenu from "./AddressMenu";
import BrowserMenu from "./BrowserMenu";
import {shortcutKey} from "./shortcuts";
import {addressDown,addressUp,type AddressPointer} from "./addressSelection";
import SetStatus from "./SetStatus";
import { ReactNode, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {listenWindow as listen} from "./windowEvents";
import Icon from "./Icons";
import { createPortal } from "react-dom";

type Vkladka = { id: number; adres: string; zagolovok: string; zakreplena: boolean; zagruzka: boolean; nazad: boolean; vpered: boolean; otlozhena?: boolean; ikonka?: string | null; privatnaya?: boolean };
type Sessiya = { vkladki: Vkladka[]; aktivnaya: number };
type Komanda = { deystvie: string; id?: number; adres?: string; zakreplena?: boolean; sprava?: boolean; pered?: number | null; procent?: number; text?: string; vpered?: boolean; dalshe?: boolean };

export default function ObychnyeVkladki({ id, nazvanie, children, novayaStranica, poisk, poleKontenta,panelZakladok=false,onToggleBookmarks=()=>{} }: {panelZakladok?:boolean;onToggleBookmarks?:()=>void; id: string | null; nazvanie: string; children: ReactNode; novayaStranica: string; poisk: string; poleKontenta: React.RefObject<HTMLDivElement> }) {
  const [sessiya, setSessiya] = useState<Sessiya | null>(null);
  const [adres, setAdres] = useState("");
  const [pravka, setPravka] = useState(false);
  const [oshibka, setOshibka] = useState("");
  const [dragOver, setDragOver] = useState<number | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; v: Vkladka } | null>(null);
  const addressPointer=useRef<AddressPointer>(null);
  const pole = useRef<HTMLInputElement>(null);
  const findInput = useRef<HTMLInputElement>(null);
  const findTimer = useRef<ReturnType<typeof setTimeout>>();
  const lastFind = useRef("");
  const [translation,setTranslation]=useState<{done:number;total:number;error?:string|null}|null>(null);
  const [toolsOpen,setToolsOpen]=useState(false);
  const [findOpen, setFindOpen] = useState(false), [findText, setFindText] = useState("");
  const [matches, setMatches] = useState({count:0, active:0});
  const lenta = useRef<HTMLDivElement>(null);
  const sessiyaRef = useRef<Sessiya | null>(null);
  const [shirina, setShirina] = useState({ predel: 230, tesno: false });
  const skolko = sessiya?.vkladki.length ?? 0;
  const zakrepleno = sessiya?.vkladki.filter((v) => v.zakreplena).length ?? 0;
  const izmerit = useCallback(() => {
    const list = lenta.current, polosa = list?.closest(".polosa") as HTMLElement | null;
    if (!list || !polosa) return;
    const tabs = sessiyaRef.current?.vkladki ?? [];
    const pinned = tabs.filter(v => v.zakreplena).length, normal = tabs.length - pinned;
    const plus = polosa.querySelector(".novaya-obychnaya") as HTMLElement | null;
    const css = getComputedStyle(polosa), group = list.parentElement!;
    const px = (s: string) => parseFloat(s) || 0;
    // Режим выбираем независимо от текущей ширины кнопки: иначе порог
    // создаёт цикл «скрыть подпись → места хватает → показать подпись».
    const compact = normal > 6 || (polosa.clientWidth - 200) / Math.max(1, normal) < 120;
    document.documentElement.dataset.tesno = compact ? "da" : "";
    const neighbours=[...polosa.children].filter(e=>e!==group&&getComputedStyle(e).position!=="fixed"&&e.getBoundingClientRect().width>0);
    const available = polosa.clientWidth - px(css.paddingLeft) - px(css.paddingRight)
      - neighbours.reduce((sum,e)=>sum+e.getBoundingClientRect().width,0)
      - px(css.columnGap)*neighbours.length - (plus?.getBoundingClientRect().width ?? 30) - 26
      - px(getComputedStyle(group).columnGap) - px(getComputedStyle(list).columnGap)*Math.max(0,tabs.length-1)
      - pinned*44;
    const width = normal ? Math.max(20, Math.min(230, Math.floor(available / normal))) : 230;
    const tight = width < 160 && normal > 3;
    setShirina(s => s.predel === width && s.tesno === tight ? s : {predel: width, tesno: tight});
  }, []);
  useLayoutEffect(izmerit, [izmerit, skolko, zakrepleno]);
  useEffect(() => {
    const polosa = lenta.current?.closest(".polosa");
    if (!polosa) return;
    const observer = new ResizeObserver(izmerit);
    observer.observe(polosa);
    for(const child of polosa.children){if(child!==lenta.current?.parentElement)observer.observe(child);}
    window.addEventListener("resize", izmerit);
    return () => { observer.disconnect(); window.removeEventListener("resize", izmerit); delete document.documentElement.dataset.tesno; };
  }, [izmerit]);
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
  const [bookmarked,setBookmarked]=useState(false);
  const [bookmarkMessage,setBookmarkMessage]=useState("");
  useEffect(()=>{let live=true;const refresh=()=>invoke<{adres:string}[]>("biblioteka_spisok",{razdel:"zakladki"}).then(rows=>{if(live)setBookmarked(rows.some(x=>x.adres===aktivnaya?.adres));}).catch(()=>{});void refresh();const off=listen("zakladki-izmenilis",refresh);return()=>{live=false;void off.then(f=>f());};},[aktivnaya?.adres]);
  async function toggleBookmark(){if(!aktivnaya)return;try{const added=await invoke<boolean>("zakladki_pereklyuchit",{adres:aktivnaya.adres,nazvanie:aktivnaya.zagolovok});setBookmarked(added);setBookmarkMessage(added?"Закладка добавлена":"Закладка удалена");}catch(e){setOshibka(String(e));}}
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
      if(e.altKey)return;
      if((e.ctrlKey||e.metaKey)&&e.shiftKey&&shortcutKey(e)==="b"){e.preventDefault();onToggleBookmarks();return;}
      const k = shortcutKey(e);
      const ctrl = e.ctrlKey || e.metaKey;
      // F5 и Ctrl+F5 обязаны работать как в любом браузере, а не только ⟳.
      if (k !== "f12" && k !== "f5" && !(ctrl && ["t", "w", "l", "u", "r", "tab", "i", "f", "d", "n"].includes(k))) return;
      e.preventDefault();
      if (document.querySelector(".zatemnenie")) return;
      if (ctrl && k === "n") {void invoke("novoe_okno",{id,privatnaya:e.shiftKey}).catch(e=>setOshibka(String(e)));return;}
      if (ctrl && k === "d") {void toggleBookmark();return;}
      if (ctrl && k === "f") { openFind(); return; }
      if (ctrl && k === "l") { focusAddress(); return; }
      if (!id || !aktivnaya) return;
      if (k === "f5") void poslat({ deystvie: e.shiftKey || ctrl ? "obnovit_bez_kesha" : "obnovit", id: aktivnaya.id });
      else if (k === "f12" || (ctrl && e.shiftKey && k === "i")) void poslat({ deystvie: "instrumenty", id: aktivnaya.id });
      else if (k === "t" && e.shiftKey) void poslat({deystvie:"vosstanovit_zakrytuyu"});
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
  }, [id, aktivnaya?.id, skolko, aktivnaya?.adres, novayaStranica,onToggleBookmarks]);
  function openFind() {
    setFindOpen(true);
    void invoke("fokus_obolochke").then(() => requestAnimationFrame(() => {findInput.current?.focus();findInput.current?.select();}));
  }
  function closeFind() {
    clearTimeout(findTimer.current);lastFind.current="";
    setFindOpen(false);
    if (aktivnaya) void poslat({deystvie:"poisk_zakryt",id:aktivnaya.id});
  }
  useEffect(() => {
    setTranslation(null);setToolsOpen(false);setFindOpen(false);setFindText("");setMatches({count:0,active:0});
  }, [id,aktivnaya?.id,aktivnaya?.adres]);
  useEffect(() => {
    const off=listen<{super_id:string;data:{tip:string;id:number;open?:boolean;count?:number;active?:number;procent?:number;start?:boolean;original?:boolean;done?:number;total?:number;error?:string|null}}>("stranica-ui",e=>{
      if(e.payload.super_id!==id||e.payload.data.id!==aktivnaya?.id)return;
      if(e.payload.data.tip==="panel-zakladok"){onToggleBookmarks();return;}
      if(e.payload.data.tip==="zakladka"){void toggleBookmark();return;}
      if(e.payload.data.tip==="perevod"){const d=e.payload.data;setTranslation(d.original?null:{done:d.done??0,total:d.total??0,error:d.error});return;}
      if(e.payload.data.tip==="devtools-error"){setOshibka(e.payload.data.error||"Не удалось открыть инструменты разработчика");return;}
      if(e.payload.data.tip==="devtools"){setToolsOpen(!!e.payload.data.open);return;}
      if(e.payload.data.tip!=="poisk")return;
      const d=e.payload.data;if(d.open)openFind();
      if(d.count!==undefined)setMatches({count:d.count,active:d.active??0});
    });
    return()=>{void off.then(f=>f());};
  },[id,aktivnaya?.id,aktivnaya?.adres,onToggleBookmarks]);
  function runFind(forward:boolean,next:boolean) {
    clearTimeout(findTimer.current);
    if(!aktivnaya)return;
    const key=JSON.stringify([id,aktivnaya.id,findText]);
    const repeat=next&&lastFind.current===key;
    lastFind.current=key;
    void poslat({deystvie:"nayti",id:aktivnaya.id,text:findText,vpered:forward,dalshe:repeat});
  }
  useEffect(() => {
    if(!findOpen||!aktivnaya)return;
    setMatches({count:0,active:0});
    findTimer.current=setTimeout(()=>runFind(true,false),120);
    return()=>clearTimeout(findTimer.current);
  },[findText,findOpen,id,aktivnaya?.id]);
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
          className={"obychnaya" + (v.id === sessiya?.aktivnaya ? " vybrana" : "") + (v.zakreplena ? " zakreplena" : "") + (v.otlozhena ? " otlozhena" : "") + (v.privatnaya?" private-tab":"") + (shirina.tesno && !v.zakreplena ? " tesno" : "") + (shirina.predel < 36 && !v.zakreplena ? " mikro" : "") + (dragOver === v.id ? " drop-before" : "")}
          onDragStart={(e) => { e.dataTransfer.setData("application/nexusbro-tab", String(v.id)); e.dataTransfer.effectAllowed = "move"; void invoke("pokazat_dvizhki", { vidno: false }); }}
          onDragOver={(e) => { if (e.dataTransfer.types.includes("application/nexusbro-tab")) { e.preventDefault(); e.stopPropagation(); setDragOver(v.id); } }}
          onDrop={(e) => void finishDrag(v.id, e)} onDragEnd={() => { setDragOver(null); void invoke("pokazat_dvizhki", { vidno: true }); }}
          onMouseDown={(e) => { if (e.button === 1) e.preventDefault(); }}
          onAuxClick={(e) => { if (e.button === 1) { e.preventDefault(); e.stopPropagation(); void poslat({ deystvie: "zakryt", id: v.id }); } }}
          onContextMenu={async (e) => { e.preventDefault(); e.stopPropagation(); const m = { x: Math.max(8, Math.min(e.clientX, window.innerWidth - 260)), y: Math.max(8, Math.min(e.clientY, window.innerHeight - 350)), v }; await invoke("pokazat_dvizhki", { vidno: false }); setMenu(m); }}
          title={(v.otlozhena ? "Спит — нажмите, чтобы загрузить\n" : "") + v.zagolovok + "\n" + v.adres + (v.zakreplena ? "\nЗакреплена" : "")} onClick={() => void poslat({ deystvie: "vybrat", id: v.id })}
          onKeyDown={(e) => { if (e.target !== e.currentTarget) return; if (e.key === "Enter" || e.key === " ") { e.preventDefault(); void poslat({ deystvie: "vybrat", id: v.id }); } }}>
          {v.privatnaya&&<span title="Приватная вкладка" aria-label="Приватная вкладка" className="private-mark">◐</span>}<span className={"favicon" + (v.zagruzka ? " loading" : "")}>{v.zagruzka ? <span className="spinner" /> : v.ikonka?.startsWith("data:image/png;base64,") ? <img src={v.ikonka} alt="" draggable={false} /> : <Icon name="globe" size={16} />}</span>
          {!v.zakreplena && <span className="obychnaya-imya">{v.zagolovok || "Новая вкладка"}</span>}
          {!v.zakreplena && <button className="zakryt-obychnuyu" title="Закрыть обычную вкладку" aria-label="Закрыть обычную вкладку"
            onClick={(e) => { e.stopPropagation(); void poslat({ deystvie: "zakryt", id: v.id }); }}><Icon name="close" size={13} /></button>}
        </div>)}
      </div><button className="novaya-obychnaya" title="Новая обычная вкладка" aria-label="Новая обычная вкладка" disabled={!id}
        onClick={() => void poslat({ deystvie: "novaya", adres: novayaStranica })}><Icon name="plus" size={18} /></button><BrowserMenu label="Варианты новой вкладки" className="new-tab-options" items={[{label:"Новая обычная вкладка",shortcut:"Ctrl+T",action:()=>poslat({deystvie:"novaya",adres:novayaStranica})},{label:"Новая приватная вкладка",action:()=>poslat({deystvie:"novaya_privatnaya",adres:novayaStranica})}]}><span aria-hidden="true">⌄</span></BrowserMenu>
      </div>
    </div>
    {aktivnaya?.privatnaya&&<div className="private-notice">Приватная вкладка · История и cookies не сохраняются после закрытия приватных вкладок. Сетевой выход — этой супер-вкладки.</div>}
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
        value={adres}
        onPointerDown={e=>{addressPointer.current=addressDown(e.currentTarget,e.button,e.clientX,e.clientY);}}
        onFocus={()=>setPravka(true)}
        onMouseUp={e=>{if(addressUp(e.currentTarget,addressPointer.current,e.clientX,e.clientY,e.detail))e.preventDefault();addressPointer.current=null;}}
        onPointerCancel={()=>{addressPointer.current=null;}}
        onContextMenu={e=>e.stopPropagation()}
        onBlur={()=>{addressPointer.current=null;setPravka(false);}}
        onChange={e=>setAdres(e.target.value)} />

      <button type="button" title={(bookmarked?"Удалить закладку":"Добавить в закладки")+" · Ctrl+D"} aria-label={bookmarked?"Удалить закладку":"Добавить в закладки"} aria-pressed={bookmarked} className={bookmarked?"bookmark-star selected":"bookmark-star"} disabled={!aktivnaya} onClick={()=>{void toggleBookmark();}}>{bookmarked?"★":"☆"}</button>
      <SetStatus id={id}/>{children}<BrowserMenu label="Меню NexusBro" items={[
       {label:"Новое окно",shortcut:"Ctrl+N",action:()=>invoke("novoe_okno",{id,privatnaya:false})},
       {label:"Новое приватное окно",shortcut:"Ctrl+Shift+N",action:()=>invoke("novoe_okno",{id,privatnaya:true})},null,
       ...([['История','Ctrl+H','istoriya'],['Закладки','Ctrl+B','zakladki'],['Загрузки','Ctrl+J','zagruzki']] as const).map(([label,shortcut,section])=>({label,shortcut,keepsHidden:true,action:()=>{window.dispatchEvent(new CustomEvent('nexusbro-library',{detail:section}));}})),null,
       {label:panelZakladok?"Скрыть панель закладок":"Показать панель закладок",shortcut:"Ctrl+Shift+B",action:()=>onToggleBookmarks()},
       {label:"Настройки NexusBro",keepsHidden:true,action:()=>{window.dispatchEvent(new Event('nexusbro-settings'));}}
      ]}><Icon name="menu" size={18}/></BrowserMenu>
      {bookmarkMessage&&<span className="sr-only" role="status">{bookmarkMessage}</span>}
      {oshibka && <span className="oshibka-navigacii" role="alert" title={oshibka}>{oshibka}</span>}
    </form>
    <BookmarkBar visible={panelZakladok} onOpen={(url,newTab)=>poslat(newTab||!aktivnaya?{deystvie:"novaya",adres:url}:{deystvie:"pereyti",id:aktivnaya.id,adres:url})}/>
    <AddressMenu input={pole} onChange={setAdres} onError={setOshibka}/>
    {translation && <div className="translation-bar" role="status"><strong>Перевод на русский</strong><span>{translation.error || (translation.total ? `${translation.done} из ${translation.total}` : "Подготавливаю текст…")}</span><button onClick={()=>{if(aktivnaya)void poslat({deystvie:"perevod_original",id:aktivnaya.id});}}>Показать оригинал</button></div>}
    {toolsOpen && <div className="tools-bar"><strong>Инструменты: {aktivnaya?.zagolovok}</strong><span title={aktivnaya?.adres}>Супер-вкладка «{nazvanie}» · {aktivnaya?.adres}</span><button aria-label="Закрыть инструменты разработчика" onClick={()=>{if(aktivnaya)void poslat({deystvie:"instrumenty",id:aktivnaya.id});}}>×</button></div>}
    {findOpen && <form className="find-bar" role="search" aria-label="Поиск на странице" onSubmit={e=>{e.preventDefault();runFind(true,true);}} onKeyDown={e=>{if(e.key==="Escape"){e.preventDefault();closeFind();}else if(e.key==="Enter"&&e.shiftKey){e.preventDefault();runFind(false,true);}}}>
      <input ref={findInput} aria-label="Найти на странице" placeholder="Найти на странице…" value={findText} onChange={e=>setFindText(e.target.value)}/>
      <span role="status">{findText ? `${matches.active} из ${matches.count}` : "Введите текст"}</span>
      <button type="button" aria-label="Предыдущее совпадение" disabled={!findText} onClick={()=>{runFind(false,true);}}>↑</button>
      <button aria-label="Следующее совпадение" disabled={!findText}>↓</button>
      <button type="button" aria-label="Закрыть поиск" onClick={closeFind}>×</button>
    </form>}
    {aktivnaya?.otlozhena && poleKontenta.current && createPortal(<div className="pusto"><Icon name="tabs" size={36} /><h1>Вкладка ждёт открытия</h1><p>Страница не загружалась после запуска. Нажмите вкладку или кнопку ниже, когда она понадобится.</p><button onClick={() => void poslat({ deystvie: "vybrat", id: aktivnaya.id })}>Загрузить вкладку</button></div>, poleKontenta.current)}
    {menu && <div className="zatemnenie-prozrachnoe" onClick={() => void zakrytMenu()} onContextMenu={(e) => { e.preventDefault(); void zakrytMenu(); }}>
      <div className="vypadajka menu-obychnyh" role="menu" aria-label="Действия с вкладкой" style={{ left: menu.x, top: menu.y, bottom: "auto" }} onClick={(e) => e.stopPropagation()}>
        <button role="menuitem" onClick={() => void zakrytMenu({ deystvie: "novaya", adres: novayaStranica })}>Новая вкладка</button>
        <button role="menuitem" onClick={() => void zakrytMenu({ deystvie: "novaya_privatnaya", adres: "about:blank" })}>Новая приватная вкладка</button>
        <button role="menuitem" onClick={() => void zakrytMenu({deystvie:"vosstanovit_zakrytuyu"})}><span>Вернуть закрытую вкладку</span><kbd>Ctrl+Shift+T</kbd></button>
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
