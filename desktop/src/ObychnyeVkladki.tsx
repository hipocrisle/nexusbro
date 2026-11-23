import BookmarkBar from "./BookmarkBar";
import "./addressSuggestions.css";
import AddressMenu from "./AddressMenu";
import {popupMenu} from "./nativeMenu";
import BrowserMenu from "./BrowserMenu";
import {shortcutKey} from "./shortcuts";
import {addressDown,addressUp,type AddressPointer} from "./addressSelection";
import {addressCompletion} from "./addressCompletion";
import SetStatus from "./SetStatus";
import {TabRouteButton,TabRouteDialog,useTabRouteChoices} from "./TabRouteSelector";
import {routeLabel,routeStateKnown,type SelectableTabRoute,type TabRouteState} from "./tabRouteTypes";
import { ReactNode, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {listenWindow as listen} from "./windowEvents";
import Icon from "./Icons";
import { createPortal } from "react-dom";

type Vkladka = TabRouteState & { browser_id?:number; memory_pending?: boolean; id: number; adres: string; zagolovok: string; zakreplena: boolean; zagruzka: boolean; nazad: boolean; vpered: boolean; otlozhena?: boolean; ikonka?: string | null; privatnaya?: boolean };
type Sessiya = { vkladki: Vkladka[]; aktivnaya: number; zakrytye?: Vkladka[] };
type Komanda = { deystvie: string;enabled?:boolean; id?: number; adres?: string; zakreplena?: boolean; sprava?: boolean; pered?: number | null; procent?: number; text?: string; vpered?: boolean; dalshe?: boolean };

export default function ObychnyeVkladki({ id, nazvanie, children, novayaStranica, poisk, poleKontenta,panelZakladok=false,onToggleBookmarks=()=>{} }: {panelZakladok?:boolean;onToggleBookmarks?:()=>void; id: string | null; nazvanie: string; children: ReactNode; novayaStranica: string; poisk: string; poleKontenta: React.RefObject<HTMLDivElement> }) {
  const [receivedSession,setReceivedSession]=useState<{owner:string|null;data:Sessiya}|null>(null);
  const sessiya=receivedSession?.owner===id?receivedSession.data:null;
  const routeSessionRevision=useRef(0);
  const setSessiya=(data:Sessiya|null)=>{routeSessionRevision.current++;setReceivedSession(data?{owner:id,data}:null);};
  const [adres, setAdres] = useState("");
  const [pravka, setPravka] = useState(false);
  const [suggestions,setSuggestions]=useState<{adres:string;nazvanie:string;istochnik:string}[]>([]);
  const [suggestionIndex,setSuggestionIndex]=useState(-1);
  const completionAllowed=useRef(false),completedValue=useRef<string|null>(null),completionRange=useRef<number|null>(null);
  const [oshibka, setOshibka] = useState("");
  const [routeDialog,setRouteDialog]=useState<{owner:string;tabId:number}|null>(null);
  const routeOwner=useRef(id),routesAlive=useRef(true);
  routeOwner.current=id;
  useEffect(()=>{routesAlive.current=true;return()=>{routesAlive.current=false;};},[]);
  useEffect(()=>{setRouteDialog(null);},[id]);
  const [dragOver, setDragOver] = useState<number | null>(null);
  const addressPointer=useRef<AddressPointer>(null);
  const addressMenuOpen=useRef(false);
  const pole = useRef<HTMLInputElement>(null);
  const findInput = useRef<HTMLInputElement>(null);
  const findTimer = useRef<ReturnType<typeof setTimeout>>();
  const lastFind = useRef("");
  const [translation,setTranslation]=useState<{done:number;total:number;auto?:boolean;error?:string|null}|null>(null);
  const [passwordOffer,setPasswordOffer]=useState<{draft_id:string;origin:string;username:string}|null>(null);
  const [savingPassword,setSavingPassword]=useState(false);
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
    setSessiya(null);
    if (!id) return;
    let zhiv = true;
    let obnovleniya = 0;
    const otpiska = listen<{ id: string; sessiya: Sessiya }>("obychnye-izmenilis", (e) => {
      if (zhiv && e.payload.id === id) { obnovleniya++; setSessiya(e.payload.sessiya); }
    });
    void otpiska.then(async()=>{
      for(let attempt=0;zhiv&&obnovleniya===0&&attempt<30;attempt++){
        try{const s=await invoke<Sessiya>("obychnye_spisok",{id});if(zhiv&&obnovleniya===0)setSessiya(s);return;}
        catch(e){if(!zhiv||obnovleniya>0)return;if(String(e).includes("ещё запускается")&&attempt<29){await new Promise(r=>setTimeout(r,500));continue;}setOshibka(String(e));return;}
      }
    });
    return () => { zhiv = false; void otpiska.then((f) => f()); };
  }, [id]);
  sessiyaRef.current = sessiya;
  const aktivnaya = sessiya?.vkladki.find((v) => v.id === sessiya.aktivnaya);
  const routeOptions=useTabRouteChoices(id,aktivnaya?.route_generation);
  const routeTarget=routeDialog?.owner===id?sessiya?.vkladki.find(tab=>tab.id===routeDialog.tabId):undefined;
  useEffect(()=>{if(routeDialog&&!routeTarget)setRouteDialog(null);},[routeDialog,routeTarget]);
  function openTabRoute(tab:Vkladka){if(id&&routeStateKnown(tab)&&!tab.memory_pending&&!tab.route_pending)setRouteDialog({owner:id,tabId:tab.id});}
  async function applyTabRoute(tabId:number,route:SelectableTabRoute,expectedGeneration:number){
    const owner=id,current=sessiyaRef.current?.vkladki.find(tab=>tab.id===tabId);
    if(!owner||routeOwner.current!==owner||!current||current.memory_pending||current.route_pending||current.route_generation!==expectedGeneration)throw Error("Состояние вкладки изменилось. Откройте выбор пути заново.");
    await invoke('tab_route_set',{id:owner,tabId,route,expectedGeneration});
    if(!routesAlive.current||routeOwner.current!==owner)return;
    const revision=routeSessionRevision.current;
    const latest=await invoke<Sessiya>('obychnye_spisok',{id:owner});
    if(routesAlive.current&&routeOwner.current===owner&&routeSessionRevision.current===revision){routeSessionRevision.current++;setReceivedSession({owner,data:latest});}
  }
  useEffect(()=>{
    let alive=true;
    type PageMenu={tip:string;id:number;token:string;x:number;y:number;items:({command_id:number;text:string;enabled:boolean}|null)[]};
    const off=listen<{super_id:string;data:PageMenu}>('stranica-ui',event=>{
      const data=event.payload.data;
      if(event.payload.super_id!==id||data.tip!=='page-context-menu')return;
      const reply=(command_id:number|null)=>invoke('obychnaya_komanda',{id,komanda:{deystvie:'page_context_choose',token:data.token,command_id}});
      if(!alive||sessiyaRef.current?.aktivnaya!==data.id){void reply(null).catch(()=>{});return;}
      const bounds=document.querySelector('.pole')?.getBoundingClientRect();
      let chosen=false;
      void popupMenu(data.items.map(item=>item?{text:item.text,enabled:item.enabled,action:async()=>{chosen=true;await reply(item.command_id);}}:null),(bounds?.left||0)+data.x,(bounds?.top||0)+data.y,message=>{if(alive)setOshibka(message);})
        .catch(error=>{if(alive)setOshibka(String(error));})
        .finally(()=>{if(!chosen)void reply(null).catch(()=>{});});
    });
    return()=>{alive=false;void off.then(stop=>stop());};
  },[id]);
  useEffect(()=>{setPasswordOffer(null);const off=listen<{super_id:string;data:{tip:string;id:number;draft_id:string;origin:string;username:string}}>('stranica-ui',e=>{if(e.payload.super_id===id&&e.payload.data.id===aktivnaya?.id&&e.payload.data.tip==='password-offer')setPasswordOffer(e.payload.data);});return()=>{void off.then(f=>f());};},[id,aktivnaya?.id]);
  const [bookmarked,setBookmarked]=useState(false);
  const [bookmarkMessage,setBookmarkMessage]=useState("");
  useEffect(()=>{let live=true;const refresh=()=>invoke<{adres:string}[]>("biblioteka_spisok",{razdel:"zakladki"}).then(rows=>{if(live)setBookmarked(rows.some(x=>x.adres===aktivnaya?.adres));}).catch(()=>{});void refresh();const off=listen("zakladki-izmenilis",refresh);return()=>{live=false;void off.then(f=>f());};},[aktivnaya?.adres]);
  async function toggleBookmark(){if(!aktivnaya)return;try{const added=await invoke<boolean>("zakladki_pereklyuchit",{adres:aktivnaya.adres,nazvanie:aktivnaya.zagolovok});setBookmarked(added);setBookmarkMessage(added?"Закладка добавлена":"Закладка удалена");}catch(e){setOshibka(String(e));}}
  useEffect(() => { if (!pravka) setAdres(aktivnaya?.adres ?? ""); }, [aktivnaya?.adres, aktivnaya?.id, pravka]);
  useEffect(()=>{
    let live=true;
    if(!pravka||!adres.trim()){setSuggestions([]);completedValue.current=null;return;}
    if(completedValue.current===adres)return;
    setSuggestions([]);setSuggestionIndex(-1);
    const timer=setTimeout(()=>{void invoke<{adres:string;nazvanie:string;istochnik:string}[]>("biblioteka_podskazki",{query:adres,limit:8}).then(rows=>{
      if(!live)return;
      rows=Array.isArray(rows)?rows:[];setSuggestions(rows);
      const input=pole.current;
      if(completionAllowed.current&&input===document.activeElement&&input?.selectionStart===adres.length&&input.selectionEnd===adres.length){
        const full=addressCompletion(adres,rows);completionAllowed.current=false;
        if(full){completedValue.current=full.value;completionRange.current=full.from;setAdres(full.value);}
      }
    }).catch(()=>{if(live)setSuggestions([]);});},80);
    return()=>{live=false;clearTimeout(timer);};
  },[adres,pravka,id,aktivnaya?.id]);
  useEffect(()=>{if(suggestionIndex>=0)document.getElementById(`address-option-${suggestionIndex}`)?.scrollIntoView({block:'nearest'});},[suggestionIndex]);
  useLayoutEffect(()=>{if(completionRange.current!==null){pole.current?.setSelectionRange(completionRange.current,adres.length);completionRange.current=null;}},[adres]);
  useEffect(()=>{
    if(!id||!sessiya)return;
    const sync=()=>{const hex=getComputedStyle(document.documentElement).getPropertyValue("--fon2").trim();if(/^#[0-9a-f]{6}$/i.test(hex))void invoke("obychnaya_komanda",{id,komanda:{deystvie:"fon_konteynera",rgb:parseInt(hex.slice(1),16)}}).catch(()=>{});};
    sync();const observer=new MutationObserver(sync);observer.observe(document.documentElement,{attributes:true,attributeFilter:["data-tema"]});
    const media=matchMedia("(prefers-color-scheme: light)");media.addEventListener("change",sync);
    return()=>{observer.disconnect();media.removeEventListener("change",sync);};
  },[id,!!sessiya]);

  useEffect(() => {
    if (!aktivnaya || aktivnaya.adres !== "about:blank") return;
    let zhiv = true;
    setAdres("about:blank");
    void invoke("fokus_obolochke").then(() => {
      if (zhiv) { pole.current?.focus(); pole.current?.select(); }
    });
    return () => { zhiv = false; };
  }, [id, aktivnaya?.id]);
  function tabMenu(v:Vkladka,x:number,y:number){
    const action=(komanda:Komanda)=>()=>poslat(komanda);
    void popupMenu([
      {text:"Новая вкладка",action:action({deystvie:"novaya",adres:novayaStranica})},
      {text:"Новая приватная вкладка",action:action({deystvie:"novaya_privatnaya",adres:"about:blank"})},
      {text:"Вернуть закрытую вкладку",enabled:!!sessiya?.zakrytye?.length,action:action({deystvie:"vosstanovit_zakrytuyu"})},
      {text:"Обновить",action:action({deystvie:"obnovit",id:v.id})},
      {text:"Дублировать",action:action({deystvie:"novaya",adres:v.adres})},
      {text:"Путь вкладки…",enabled:routeStateKnown(v)&&!v.memory_pending&&!v.route_pending,action:()=>openTabRoute(v)},
      {text:v.zakreplena?"Открепить":"Закрепить",action:action({deystvie:"zakrepit",id:v.id,zakreplena:!v.zakreplena})},
      {text:"Исходный код страницы",action:action({deystvie:"iskhodnik",id:v.id})},
      {text:"Инструменты разработчика",action:action({deystvie:"instrumenty",id:v.id})},null,
      {text:"Закрыть вкладку",action:action({deystvie:"zakryt",id:v.id})},
      {text:"Закрыть другие вкладки",enabled:!!sessiya?.vkladki.some(t=>t.id!==v.id&&!t.zakreplena),action:action({deystvie:"zakryt_drugie",id:v.id,sprava:false})},
      {text:"Закрыть вкладки справа",enabled:!!sessiya?.vkladki.slice(sessiya.vkladki.findIndex(t=>t.id===v.id)+1).some(t=>!t.zakreplena),action:action({deystvie:"zakryt_drugie",id:v.id,sprava:true})}
    ],x,y,setOshibka).catch(e=>setOshibka(String(e)));
  }
  async function poslat(komanda: Komanda) {
    if (!id) return;
    const target = sessiyaRef.current?.vkladki.find(tab => tab.id === komanda.id);
    if (target?.memory_pending && ["pereyti", "obnovit", "obnovit_bez_kesha", "nazad", "vpered", "iskhodnik", "instrumenty", "issledovat", "perevesti", "nayti"].includes(komanda.deystvie)) {
      setOshibka("Завершаем выгрузку… Страница откроется после её завершения.");
      return;
    }
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
    const off=listen<{super_id:string;data:{tip:string;id:number;open?:boolean;auto?:boolean;count?:number;active?:number;procent?:number;start?:boolean;original?:boolean;done?:number;total?:number;error?:string|null}}>("stranica-ui",e=>{
      if(e.payload.super_id!==id||e.payload.data.id!==aktivnaya?.id)return;
      if(e.payload.data.tip==="tab-search"){window.dispatchEvent(new Event("nexusbro-tab-search"));return;}
      if(e.payload.data.tip==="panel-zakladok"){onToggleBookmarks();return;}
      if(e.payload.data.tip==="zakladka"){void toggleBookmark();return;}
      if(e.payload.data.tip==="perevod"){const d=e.payload.data;setTranslation(d.original&&!d.auto?null:{done:d.done??0,total:d.total??0,auto:!!d.auto,error:d.error});return;}
      if(e.payload.data.tip==="devtools-error"){setOshibka(e.payload.data.error||"Не удалось открыть инструменты разработчика");return;}
      if(e.payload.data.tip==="devtools"){setToolsOpen(!!e.payload.data.open);return;}
      if(e.payload.data.tip!=="poisk")return;
      const d=e.payload.data;if(d.open)openFind();
      if(d.count!==undefined)setMatches({count:d.count,active:d.active??0});
    });
    // После смены Super/страницы повторяем нативное состояние docked DevTools:
    // одноразовый старый event мог прийти до регистрации нового обработчика.
    if(id&&aktivnaya)void off.then(()=>invoke("obychnaya_komanda",{id,komanda:{deystvie:"obnovit_interfeys"}})).catch(()=>{});
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
          onContextMenu={e=>{e.preventDefault();e.stopPropagation();tabMenu(v,e.clientX,e.clientY);}}
          title={(v.memory_pending ? "Завершаем выгрузку…\n" : v.otlozhena ? "Страница не загружена в память — нажмите, чтобы загрузить\n" : "") + v.zagolovok + "\n" + v.adres + (v.zakreplena ? "\nЗакреплена" : "") + "\nПуть: " + routeLabel(v.route,routeOptions.choices) + (v.route_pending ? " · Применяем…" : v.route_error ? " · " + v.route_error : "")} onClick={() => void poslat({ deystvie: "vybrat", id: v.id })}
          onKeyDown={(e) => { if (e.target !== e.currentTarget) return; if (e.key === "Enter" || e.key === " ") { e.preventDefault(); void poslat({ deystvie: "vybrat", id: v.id }); } }}>
          {v.privatnaya&&<span title="Приватная вкладка" aria-label="Приватная вкладка" className="private-mark">◐</span>}<span className={"favicon" + (v.zagruzka ? " loading" : "")}>{v.zagruzka ? <span className="spinner" /> : v.ikonka?.startsWith("data:image/png;base64,") ? <img src={v.ikonka} alt="" draggable={false} /> : <Icon name="globe" size={16} />}</span>
          {!v.zakreplena && <span className="obychnaya-imya">{v.zagolovok || "Новая вкладка"}</span>}
          {!v.zakreplena && <button className="zakryt-obychnuyu" title="Закрыть обычную вкладку" aria-label="Закрыть обычную вкладку"
            onClick={(e) => { e.stopPropagation(); void poslat({ deystvie: "zakryt", id: v.id }); }}><Icon name="close" size={13} /></button>}
        </div>)}
      </div><button className="novaya-obychnaya" title="Новая обычная вкладка" aria-label="Новая обычная вкладка" disabled={!id}
        onClick={() => void poslat({ deystvie: "novaya", adres: novayaStranica })}><Icon name="plus" size={18} /></button><BrowserMenu label="Варианты новой вкладки" className="new-tab-options" items={[{label:"Новая обычная вкладка",shortcut:"Ctrl+T",action:()=>poslat({deystvie:"novaya",adres:novayaStranica})},{label:"Новая приватная вкладка",action:()=>poslat({deystvie:"novaya_privatnaya",adres:novayaStranica})}]}><Icon name="chevronDown" size={14}/></BrowserMenu>
      </div>
    </div>
    {aktivnaya?.privatnaya&&<div className="private-notice">Приватная вкладка · История и cookies не сохраняются после закрытия приватных вкладок. Сетевой выход — этой супер-вкладки.</div>}
    <form className="navigaciya" onSubmit={(e) => {
      e.preventDefault();
      if (!aktivnaya || aktivnaya.memory_pending) return;
      const text = (suggestionIndex>=0?suggestions[suggestionIndex]?.adres??adres:pole.current?.value??adres).trim();
      const search = poisk === "google" ? "https://www.google.com/search?q=" : poisk === "yandex" ? "https://yandex.ru/search/?text=" : "https://duckduckgo.com/?q=";
      const url = !text ? "about:blank" : /^[a-z][a-z0-9+.-]*:/i.test(text) ? text : !/\s/.test(text) && (text.includes(".") || /^localhost(?::|\/|$)/i.test(text)) ? "https://" + text : search + encodeURIComponent(text);
      void poslat({ deystvie: "pereyti", id: aktivnaya.id, adres: url });
      pole.current?.blur();
    }}>
      <button type="button" title="Назад" disabled={!aktivnaya?.nazad||!!aktivnaya.memory_pending} onClick={() => void poslat({ deystvie: "nazad", id: aktivnaya?.id })}><Icon name="back" size={17} /></button>
      <button type="button" title="Вперёд" disabled={!aktivnaya?.vpered||!!aktivnaya.memory_pending} onClick={() => void poslat({ deystvie: "vpered", id: aktivnaya?.id })}><Icon name="forward" size={17} /></button>
      <button type="button" title="Обновить страницу" disabled={!aktivnaya||aktivnaya.memory_pending} onClick={() => void poslat({ deystvie: "obnovit", id: aktivnaya?.id })}><Icon name="reload" size={17} /></button>
      <input ref={pole} aria-label="Адрес страницы" placeholder="Введите адрес" disabled={!aktivnaya||aktivnaya.memory_pending} role="combobox" aria-autocomplete="both" aria-controls="address-history" aria-expanded={pravka&&suggestions.length>0} aria-activedescendant={suggestionIndex>=0?`address-option-${suggestionIndex}`:undefined} autoComplete="off" spellCheck={false}
        value={adres}
        onPointerDown={e=>{addressPointer.current=addressDown(e.currentTarget,e.button,e.clientX,e.clientY);}}
        onFocus={()=>setPravka(true)}
        onKeyDown={e=>{
          if(e.ctrlKey||e.altKey||e.metaKey)return;
          if((e.key==='ArrowDown'||e.key==='ArrowUp')&&suggestions.length){
            e.preventDefault();
            setSuggestionIndex(previous=>e.key==='ArrowDown'?(previous+1)%suggestions.length:(previous<=0?suggestions.length-1:previous-1));
          }else if(e.key==='Escape'){e.preventDefault();setSuggestions([]);setSuggestionIndex(-1);setPravka(false);}
        }}
        onMouseUp={e=>{if(addressUp(e.currentTarget,addressPointer.current,e.clientX,e.clientY,e.detail))e.preventDefault();addressPointer.current=null;}}
        onPointerCancel={()=>{addressPointer.current=null;}}
        onContextMenu={e=>e.stopPropagation()}
        onBlur={()=>{addressPointer.current=null;if(!addressMenuOpen.current)setPravka(false);}}
        onChange={e=>{setPravka(true);setSuggestionIndex(-1);completedValue.current=null;completionAllowed.current=(e.nativeEvent as InputEvent).inputType==='insertText';setAdres(e.target.value);}} />

      <button type="button" title={(bookmarked?"Удалить закладку":"Добавить в закладки")+" · Ctrl+D"} aria-label={bookmarked?"Удалить закладку":"Добавить в закладки"} aria-pressed={bookmarked} className={bookmarked?"bookmark-star selected":"bookmark-star"} disabled={!aktivnaya} onClick={()=>{void toggleBookmark();}}>{bookmarked?"★":"☆"}</button>
      <TabRouteButton tab={aktivnaya} choices={routeOptions.choices} onOpen={()=>{if(aktivnaya)openTabRoute(aktivnaya);}}/><SetStatus id={id}/>{children}<BrowserMenu label="Меню NexusBro" items={[
       {label:`Удалить данные супер-вкладки «${nazvanie}»…`,shortcut:"Ctrl+Shift+Delete",keepsHidden:true,action:()=>{window.dispatchEvent(new CustomEvent("nexusbro-clear-data"));}},null,
       {label:"Поиск по вкладкам",shortcut:"Ctrl+Shift+A",keepsHidden:true,action:()=>window.dispatchEvent(new Event("nexusbro-tab-search"))},
       {label:"Новое окно",shortcut:"Ctrl+N",action:()=>invoke("novoe_okno",{id,privatnaya:false})},
       {label:"Новое приватное окно",shortcut:"Ctrl+Shift+N",action:()=>invoke("novoe_okno",{id,privatnaya:true})},null,
       ...([['История','Ctrl+H','istoriya'],['Закладки','Ctrl+B','zakladki'],['Загрузки','Ctrl+J','zagruzki']] as const).map(([label,shortcut,section])=>({label,shortcut,keepsHidden:true,action:()=>{window.dispatchEvent(new CustomEvent('nexusbro-library',{detail:section}));}})),null,
       {label:panelZakladok?"Скрыть панель закладок":"Показать панель закладок",shortcut:"Ctrl+Shift+B",action:()=>onToggleBookmarks()},
       {label:"Пароли, аккаунт и синхронизация",keepsHidden:true,action:()=>window.dispatchEvent(new Event("nexusbro-security"))},
       {label:"Настройки NexusBro",keepsHidden:true,action:()=>{window.dispatchEvent(new Event('nexusbro-settings'));}}
      ]}><Icon name="menu" size={18}/></BrowserMenu>
      {bookmarkMessage&&<span className="sr-only" role="status">{bookmarkMessage}</span>}
      {oshibka && <span className="oshibka-navigacii" role="alert" title={oshibka}>{oshibka}</span>}
    </form>
    {pravka&&suggestions.length>0&&<div id="address-history" className="address-suggestions" role="listbox" aria-label="Подсказки адреса">
      {suggestions.map((row,index)=><button type="button" role="option" id={`address-option-${index}`} key={row.adres} aria-selected={suggestionIndex===index} onMouseDown={e=>e.preventDefault()} onClick={()=>{if(!aktivnaya)return;setAdres(row.adres);setSuggestions([]);setSuggestionIndex(-1);pole.current?.blur();void poslat({deystvie:"pereyti",id:aktivnaya.id,adres:row.adres});}}><span>{row.nazvanie||row.adres}</span><small>{row.adres}</small></button>)}
    </div>}
    <BookmarkBar visible={panelZakladok} onOpen={(url,newTab)=>poslat(newTab||!aktivnaya?{deystvie:"novaya",adres:url}:{deystvie:"pereyti",id:aktivnaya.id,adres:url})}/>
    <AddressMenu input={pole} onChange={setAdres} onError={setOshibka} onMenuState={open=>{addressMenuOpen.current=open;}}/>
    {routeTarget&&routeDialog?.owner===id&&id&&<TabRouteDialog key={`${id}:${routeTarget.id}`} owner={id} tab={routeTarget} choices={routeOptions.choices} choicesError={routeOptions.error} onRefresh={()=>void routeOptions.refresh()} onApply={(route,generation)=>applyTabRoute(routeTarget.id,route,generation)} onClose={()=>setRouteDialog(null)} onError={setOshibka}/>}
    {passwordOffer&&<div className="password-offer" role="status"><span>Сохранить логин <strong>{passwordOffer.username}</strong> для {passwordOffer.origin}?</span><button disabled={savingPassword} onClick={async()=>{setSavingPassword(true);try{await invoke('security_passwords',{action:'save_draft',data:{draft_id:passwordOffer.draft_id,username:passwordOffer.username}});setPasswordOffer(null);}catch(e){setOshibka(String(e));}finally{setSavingPassword(false);}}}>Сохранить пароль</button><button onClick={()=>{void invoke('security_passwords',{action:'discard_draft',data:{draft_id:passwordOffer.draft_id}}).catch(()=>{});setPasswordOffer(null);}}>Не сейчас</button></div>}
    {translation && <div className="translation-bar" role="status"><strong>Перевод на русский</strong><span>{translation.error || (translation.total ? `${translation.done} из ${translation.total}` : "Подготавливаю текст…")}</span>{translation.error&&<button onClick={()=>{if(aktivnaya)void poslat({deystvie:"perevesti",id:aktivnaya.id});}}>Повторить</button>}<label className="translation-auto" title="Переводить страницы этого сайта в этой вкладке до выключения режима или закрытия вкладки"><input type="checkbox" checked={!!translation.auto} onChange={e=>{if(aktivnaya)void poslat({deystvie:"perevod_avto",id:aktivnaya.id,enabled:e.target.checked});}}/>Автоперевод этого сайта</label><button onClick={()=>{if(aktivnaya)void poslat({deystvie:"perevod_original",id:aktivnaya.id});}}>Показать оригинал и выключить</button></div>}
    {toolsOpen && <div className="tools-bar"><strong>Инструменты: {aktivnaya?.zagolovok}</strong><span title={aktivnaya?.adres}>Супер-вкладка «{nazvanie}» · {aktivnaya?.adres}</span><button aria-label="Закрыть инструменты разработчика" onClick={()=>{if(aktivnaya)void poslat({deystvie:"instrumenty",id:aktivnaya.id});}}>×</button></div>}
    {findOpen && <form className="find-bar" role="search" aria-label="Поиск на странице" onSubmit={e=>{e.preventDefault();runFind(true,true);}} onKeyDown={e=>{if(e.key==="Escape"){e.preventDefault();closeFind();}else if(e.key==="Enter"&&e.shiftKey){e.preventDefault();runFind(false,true);}}}>
      <input ref={findInput} aria-label="Найти на странице" placeholder="Найти на странице…" value={findText} onChange={e=>setFindText(e.target.value)}/>
      <span role="status">{findText ? `${matches.active} из ${matches.count}` : "Введите текст"}</span>
      <button type="button" aria-label="Предыдущее совпадение" disabled={!findText} onClick={()=>{runFind(false,true);}}>↑</button>
      <button aria-label="Следующее совпадение" disabled={!findText}>↓</button>
      <button type="button" aria-label="Закрыть поиск" onClick={closeFind}>×</button>
    </form>}
    {aktivnaya?.browser_id===0&&aktivnaya.route_error&&!aktivnaya.route_pending&&poleKontenta.current&&createPortal(<div className="pusto" role="alert"><Icon name="globe" size={36}/><h1>Выбранный путь недоступен</h1><p>{aktivnaya.route_error}</p><button onClick={()=>openTabRoute(aktivnaya)}>Выбрать путь</button></div>,poleKontenta.current)}
    {!aktivnaya?.route_error&&(aktivnaya?.otlozhena||aktivnaya?.memory_pending) && poleKontenta.current && createPortal(<div className="pusto" role={aktivnaya.memory_pending?"status":undefined}><Icon name="tabs" size={36} /><h1>{aktivnaya.memory_pending?"Завершаем выгрузку…":"Вкладка ждёт открытия"}</h1><p>{aktivnaya.memory_pending?"Страница откроется после завершения выгрузки.":"Страница сейчас не загружена в память. Нажмите вкладку или кнопку ниже, когда она понадобится."}</p>{!aktivnaya.memory_pending&&<button onClick={() => void poslat({ deystvie: "vybrat", id: aktivnaya.id })}>Загрузить вкладку</button>}</div>, poleKontenta.current)}

  </>;
}
