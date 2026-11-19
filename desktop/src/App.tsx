import {editableTarget} from "./addressSelection";
import {popupMenu} from "./nativeMenu";
import SecuritySettings from "./SecuritySettings";
import Biblioteka from "./Biblioteka";
import Vybor from "./Vybor";
import { CSSProperties, useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import Icon from "./Icons";
import Blokirovki, { Zhurnal } from "./Blokirovki";
import {listenWindow as listen} from "./windowEvents";
import NastrojkiOkno, { NastrojkiApp, poUmolchaniyu, shrifty, Vyhod } from "./Nastrojki";
import ObychnyeVkladki from "./ObychnyeVkladki";
import { proverit_obnovlenie } from "./updater";
import "./superColors.css";
import {useSuperCloud,SuperCloudBadge,SuperCloudHeading} from "./SuperCloud";

const superColors = [{value:"",label:"Без цвета"},{value:"#5b9ee8",label:"Синий"},{value:"#56b6a2",label:"Бирюзовый"},{value:"#72b566",label:"Зелёный"},{value:"#d4ae55",label:"Золотой"},{value:"#dc9365",label:"Оранжевый"},{value:"#d77d8f",label:"Розовый"},{value:"#b48ad8",label:"Фиолетовый"}];
function superColor(value?:string){return superColors.some(c=>c.value===value)?value:undefined;}

type Nastrojki = {
  id: string;
  nazvanie: string;
  poryadok?: number;
  proxy: string;
  proxy_uzel?: string;
  adres: string;
  rezat_reklamu: boolean;
  pustaya: boolean;
  privatnaya?: boolean;
  cvet?: string;
};
type Vidno = Nastrojki & { zapushchena: boolean; sohranena: boolean };
type Otvet = { ok: boolean; soobshchenie: string };

export default function App() {
  useEffect(()=>{if(new URLSearchParams(location.search).has("okno"))void invoke("okno_gotovo").catch(e=>setOshibka(String(e)));},[]);
  const [versiya, setVersiya] = useState("");
  const [dragSuper, setDragSuper] = useState<string | null>(null);
  const [zashchita, setZashchita] = useState<Zhurnal | null>(null);
  const zhurnalOtkryt = zashchita !== null;
  useEffect(() => {
    if (!zhurnalOtkryt) return;
    let active = true;
    const timer = setInterval(() => { const id = aktivnayaRef.current; if (id) void invoke<Zhurnal>("zhurnal_rezki", { id }).then((j) => { if (active) setZashchita(j); }).catch(() => {}); }, 1000);
  return () => { active = false; clearInterval(timer); };
  }, [zhurnalOtkryt]);
  const [nastrojki, setNastrojki] = useState<NastrojkiApp>(poUmolchaniyu);
  const nastrojkiRef = useRef(nastrojki);
  const [nastroykiOtkryty, setNastroykiOtkryty] = useState(false);
  const [securityOpen,setSecurityOpen]=useState(false);
  const cloud=useSuperCloud(securityOpen);
  const [cloudSaved,setCloudSaved]=useState<Nastrojki[]>([]);
  useEffect(()=>{const open=()=>{void invoke("pokazat_dvizhki",{vidno:false}).then(()=>{setNastroykiOtkryty(false);setSecurityOpen(true);}).catch(e=>setOshibka(String(e)));};window.addEventListener("nexusbro-security",open);return()=>window.removeEventListener("nexusbro-security",open);},[]);
  useEffect(()=>{const open=()=>{void invoke("pokazat_dvizhki",{vidno:false}).then(()=>setNastroykiOtkryty(true));};window.addEventListener("nexusbro-settings",open);return()=>window.removeEventListener("nexusbro-settings",open);},[]);
  useEffect(()=>{const off=listen<NastrojkiApp>("nastrojki-izmenilis",e=>panel(e.payload));return()=>{void off.then(f=>f());};},[]);
  const [tyanem, setTyanem] = useState(false);
  const skrytiePriPeretaskivanii = useRef<Promise<unknown>>(Promise.resolve());
  const ocheredZapisi = useRef<Promise<unknown>>(Promise.resolve());
  async function sohranitNastrojki(n: NastrojkiApp) {
    const previous = nastrojkiRef.current;
    panel(n);
    const next = ocheredZapisi.current.catch(() => {}).then(() => invoke<NastrojkiApp>("nastrojki_sohranit", { nastrojki: n }));
    ocheredZapisi.current = next;
    try {
      const saved = await next;
      if (nastrojkiRef.current === n) panel(saved);
    } catch (e) {
      if (nastrojkiRef.current === n) panel(previous);
      throw e;
    }
  }
  function panel(n: NastrojkiApp) {
    nastrojkiRef.current = n;
    setNastrojki(n);
  }

  useEffect(() => {
    void getVersion().then(setVersiya).catch(() => {});
    void invoke<NastrojkiApp>("nastrojki_poluchit").then((n) => {
      panel(n);
      if (n.avtoobnovleniya) void proverit_obnovlenie();
    }).catch((e) => setOshibka(String(e)));
  }, []);
  useEffect(() => {
    const root = document.documentElement;
    root.dataset.tema = nastrojki.tema; root.dataset.plotnost = nastrojki.plotnost;
    root.style.setProperty("--font-ui", shrifty[nastrojki.shrift] || shrifty.system);
    root.style.setProperty("--font-size-ui", nastrojki.razmer_shrifta + "px");
  }, [nastrojki.tema, nastrojki.plotnost, nastrojki.shrift, nastrojki.razmer_shrifta]);
  const [vkladki, setVkladki] = useState<Vidno[]>([]);
  const [aktivnaya, setAktivnaya] = useState<string | null>(null);
  const aktivnayaRef = useRef<string | null>(null);
  const [dvizhokEst, setDvizhokEst] = useState(true);
  const [dialog, setDialog] = useState(false);

  // Esc закрывает окна оболочки. Формы Tauri-окна (CEF) нажатие не получают,
  // а без этого выход был только мышью — на окне без видимой кнопки это ловушка.
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      if (nastroykiOtkryty) { setNastroykiOtkryty(false); void invoke("pokazat_dvizhki", { vidno: true }); return; }
      if (dialog) void zakrytDialog();
    };
    document.addEventListener("keydown", key);
    return () => document.removeEventListener("keydown", key);
  }, [nastroykiOtkryty, dialog]);
  const [pravka, setPravka] = useState<Nastrojki | null>(null);
  // Вопрос «сохранить вкладку?»: null = не спрашиваем, иначе обещание.
  const [vopros, setVopros] = useState<{
    nazvanie: string;
    razreshit: (sohranit: boolean | null) => void;
  } | null>(null);
  // Меню сохранённых вкладок у кнопки «+».
  const [menuSohranennyh, setMenuSohranennyh] = useState<Nastrojki[] | null>(null);
  // Итог ручной проверки обновлений — показываем у кнопки.
  const [obnovlenie, setObnovlenie] = useState("");
  // Текст диагностики — для копирования и отправки разработчику.
  const [diagnostika, setDiagnostika] = useState<string | null>(null);
  // Контекстное меню супервкладки (правый клик).
  // Прогресс скачивания обновления: байты и общий размер (0 = неизвестен).
  const [progress, setProgress] = useState<{ polucheno: number; vsego: number } | null>(null);
  const [oshibka, setOshibka] = useState("");
  const [zanyat, setZanyat] = useState(false);
  const [zarezano, setZarezano] = useState(0);
  const mesto = useRef<HTMLDivElement>(null);

  // ⛔ Движок — чужое окно, он НЕ часть нашей вёрстки. Поэтому каждый раз
  // сообщаем в ядро точные размеры пустого места под полосой вкладок.
  const soobshchitMesto = useCallback(() => {
    const el = mesto.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    const k = window.devicePixelRatio || 1;
    invoke("zadat_mesto", {
      x: Math.round(r.left * k),
      y: Math.round(r.top * k),
      w: Math.round(r.width * k),
      h: Math.round(r.height * k),
    });
  }, []);

  useEffect(() => {
    invoke<boolean>("dvizhok_est").then(setDvizhokEst);
    invoke<Vidno[]>("spisok").then(setVkladki);
    void invoke<Nastrojki[]>("sohranennye").then(rows=>setCloudSaved(Array.isArray(rows)?rows:[])).catch(()=>{});
    invoke<string | null>("aktivnaya").then(setAktivnaya);
    soobshchitMesto();

    window.addEventListener("resize", soobshchitMesto);
    const observer = new ResizeObserver(soobshchitMesto);
    if (mesto.current) observer.observe(mesto.current);
    // ядро само сообщает, когда список изменился
    const otpiskaOshibki = listen<string>("oshibka-dvizhka", (e) => {
      setOshibka(e.payload);
    });
    const otpiska = listen<string>("vkladki-izmenilis", async () => {
      const [rows,saved]=await Promise.all([invoke<Vidno[]>("spisok"),invoke<Nastrojki[]>("sohranennye")]);
      setVkladki(rows);setCloudSaved(Array.isArray(saved)?saved:[]);cloud.refresh();
      setAktivnaya(await invoke<string | null>("aktivnaya"));
      soobshchitMesto();
    });
    // Счётчик резки — редко, раз в три секунды: чаще незачем.
    const chasy = setInterval(async () => {
      const id = aktivnayaRef.current;
      if (!id) return;
      const [rez] = await invoke<[number, number]>("schet_rezki", { id });
      setZarezano(rez);
    }, 3000);
    return () => {
      window.removeEventListener("resize", soobshchitMesto);
      observer.disconnect();
      otpiska.then((f) => f());
      otpiskaOshibki.then((f) => f());
      clearInterval(chasy);
    };
  }, [soobshchitMesto]);

  async function dobavit(nazvanie: string, proxy: string, adres: string, rezat: boolean, pustaya: boolean, proxy_uzel: string, privatnaya=false, cvet="") {
    setZanyat(true);
    setOshibka("");
    const id = (privatnaya?"pvt-":"sv") + Date.now().toString(36);
    try {
      const o = await invoke<Otvet>("otkryt", {
        nastrojki: {
          id,
          nazvanie,
          proxy,
          proxy_uzel,
          adres: adres || "about:blank",
          rezat_reklamu: rezat,
          pustaya,
          privatnaya,
          cvet,
        },
      });
      if (!o.ok) {
        // ⛔ Ошибку рисуем ПОД полосой вкладок: за закрытым диалогом её не видно,
        // и поломка выглядит как вечное «Запускаю…».
        setOshibka(o.soobshchenie);
        return;
      }
      setVkladki(await invoke<Vidno[]>("spisok"));
      setAktivnaya(id);
      setTimeout(soobshchitMesto, 100);
    } finally {
      setZanyat(false);
      setDialog(false);
      // Движки прятали, пока диалог открыт, — возвращаем активный.
      await invoke("pokazat_dvizhki", { vidno: true });
    }
  }

  // Диалог рисуется внутри нашего окна, а поверх его лежит чужое окно
  // движка: без скрытия диалог не видно (10.11.2026).
  async function otkrytDialog() {
    setMenuSohranennyh(null);
    await invoke("pokazat_dvizhki", { vidno: false });
    setDialog(true);
  }

  async function zakrytMenyu() {
    setMenuSohranennyh(null);
    await invoke("pokazat_dvizhki", { vidno: true });
  }
  async function zakrytDialog() {
    setDialog(false);
    setPravka(null);
    await invoke("pokazat_dvizhki", { vidno: true });
  }

  // Правка супер-вкладки: тот же диалог, но с её настройками.
  async function nachatPravku(n: Nastrojki) {
    setOshibka("");
    await invoke("pokazat_dvizhki", { vidno: false });
    setPravka(n);
    setDialog(true);
  }

  async function pravit(nazvanie: string, proxy: string, adres: string, rezat: boolean, pustaya: boolean, proxy_uzel: string, _privatnaya=false, cvet="") {
    if (!pravka) return;
    setZanyat(true);
    setOshibka("");
    let success = false;
    try {
      const o = await invoke<Otvet>("izmenit", {
        nastrojki: { ...pravka, nazvanie, proxy, proxy_uzel, cvet, adres: adres || "about:blank", rezat_reklamu: rezat, pustaya },
      });
      if (!o.ok) {
        setOshibka(o.soobshchenie);
        return;
      }
      setVkladki(await invoke<Vidno[]>("spisok"));
      setTimeout(soobshchitMesto, 100);
      success = true;
    } catch (e) { setOshibka(String(e)); }
    finally {
      setZanyat(false);
      if (success) { setDialog(false); setPravka(null); }
      if (success) await invoke("pokazat_dvizhki", { vidno: true });
    }
  }

  async function pereklyuchit(id: string) {
    const vk = vkladki.find((v) => v.id === id);
    // Если движок ещё не поднят, это займёт несколько секунд — говорим об этом.
    if (vk && !vk.zapushchena) setZanyat(true);
    setOshibka("");
    const o = await invoke<Otvet>("pereklyuchit", { id });
    setZanyat(false);
    if (!o.ok) {
      setOshibka(o.soobshchenie);
      return;
    }
    setAktivnaya(id);
    setVkladki(await invoke<Vidno[]>("spisok"));
    soobshchitMesto();
  }

  async function zakrytVkladku(id: string, nazvanie: string, e?: React.MouseEvent) {
    e?.stopPropagation();
    // Вкладка с данными (входы, история) спрашивает: сохранить или нет.
    // Свежая закрывается сразу, без вопросов.
    let sohranit = !!vkladki.find((v) => v.id === id)?.sohranena;
    if (!sohranit && await invoke<boolean>("nuzhno_sohranenie", { id })) {
      // Вопрос рисуется в нашем окне, а поверх лежит чужое окно движка —
      // иначе диалог не видно (10.11.2026, поймал user).
      await invoke("pokazat_dvizhki", { vidno: false });
      let otvet: boolean | null = null;
      try {
        otvet = await voprosSohraneniya(nazvanie);
      } finally {
        await invoke("pokazat_dvizhki", { vidno: true });
      }
      if (otvet === null) return;
      sohranit = otvet;
    }
    await invoke<Otvet>("zakryt", { id, sohranit });
    const spisok = await invoke<Vidno[]>("spisok");
    setVkladki(spisok);
    setAktivnaya(await invoke<string | null>("aktivnaya"));
  }

  // Трёхкнопочный вопрос «сохранить?» — window.confirm умеет только два.
  function voprosSohraneniya(nazvanie: string): Promise<boolean | null> {
    return new Promise((razreshit) => {
      setVopros({ nazvanie, razreshit: (otvet) => {
        setVopros(null);
        razreshit(otvet);
      } });
    });
  }

  async function otkrytSohranennuyu(id: string) {
    setMenuSohranennyh(null);
    try {
      const o = await invoke<Otvet>("vosstanovit", { id });
      if (!o.ok) {
        setOshibka(o.soobshchenie);
        return;
      }
      setVkladki(await invoke<Vidno[]>("spisok"));
      setAktivnaya(id);
      soobshchitMesto();
    } finally {
      await invoke("pokazat_dvizhki", { vidno: true });
    }
  }

  async function udalitSohranennuyu(id: string, nazvanie: string, e: React.MouseEvent) {
    e.stopPropagation();
    if (!window.confirm(`Удалить «${nazvanie}» из сохранённых?\n\nУ закрытой супер-вкладки профиль со входами будет стёрт. Открытая продолжит работать.`)) {
      return;
    }
    await invoke<Otvet>("udalit_sohranennuyu", { id });
    setMenuSohranennyh(await invoke<Nastrojki[]>("sohranennye"));
  }

  useEffect(() => {
    aktivnayaRef.current = aktivnaya;
  }, [aktivnaya]);

  async function otkrytZashchitu() {
    if (!aktivnaya) return;
    await invoke("pokazat_dvizhki", { vidno: false });
    try { setZashchita(await invoke("zhurnal_rezki", { id: aktivnaya })); }
    catch (e) { setOshibka(String(e)); await invoke("pokazat_dvizhki", { vidno: true }); }
  }
  async function perenestiSuper(pered: string | null, e: React.DragEvent) {
    e.preventDefault(); e.stopPropagation(); setDragSuper(null);
    const id = e.dataTransfer.getData("application/nexusbro-super");
    try { if (id && id !== pered) { await invoke("peremestit_super", { id, pered }); setVkladki(await invoke<Vidno[]>("spisok")); } }
    catch (err) { setOshibka(String(err)); }
    finally { await invoke("pokazat_dvizhki", { vidno: true }); }
  }
  const cloudOpen=vkladki.filter(v=>cloud.selected(v.id,v.privatnaya));
  const localOpen=vkladki.filter(v=>!cloud.selected(v.id,v.privatnaya));
  const openedIds=new Set(vkladki.map(v=>v.id));
  const cloudClosed=cloudSaved.filter(v=>!openedIds.has(v.id)&&cloud.selected(v.id,v.privatnaya));
  const localClosed=cloudSaved.filter(v=>!v.privatnaya&&!v.id.startsWith("pvt-")&&!openedIds.has(v.id)&&!cloud.selected(v.id,v.privatnaya));
  const hasCloud=cloudOpen.length+cloudClosed.length>0;
  function renderSuper(v:Vidno){return (
            <button
              key={v.id}
              data-super-id={v.id}
              data-cloud-state={cloud.state(v.id,v.privatnaya)}
              className={"vkladka" + (v.id === aktivnaya ? " aktivna" : "") + (dragSuper === v.id ? " drop-before" : "") + (superColor(v.cvet) ? " super-colored" : "") + (v.privatnaya ? " super-private" : "") + (cloud.selected(v.id,v.privatnaya)?" super-cloud-selected":"")}
              style={superColor(v.cvet) ? {"--super-color":v.cvet} as CSSProperties : undefined}
              aria-label={v.nazvanie + (v.privatnaya ? " — приватная супер-вкладка" : "")}
              draggable
              onDragStart={(e) => { e.dataTransfer.setData("application/nexusbro-super", v.id); e.dataTransfer.effectAllowed = "move"; void invoke("pokazat_dvizhki", { vidno: false }); }}
              onDragOver={(e) => { if (e.dataTransfer.types.includes("application/nexusbro-super")) { e.preventDefault(); e.stopPropagation(); setDragSuper(v.id); } }}
              onDrop={(e) => void perenestiSuper(v.id, e)}
              onDragEnd={() => { setDragSuper(null); void invoke("pokazat_dvizhki", { vidno: true }); }}
              onClick={() => pereklyuchit(v.id)}
              onContextMenu={async (e) => {
                e.preventDefault();
                e.stopPropagation();
                void popupMenu([
                  {text:"Настройки",action:()=>nachatPravku(v)},
                  {text:"Сохранить",action:async()=>{await invoke("sohranit_vkladku",{id:v.id});setVkladki(await invoke<Vidno[]>("spisok"));}},
                  {text:"Сохранить и закрыть",action:async()=>{await invoke("zakryt",{id:v.id,sohranit:true});setVkladki(await invoke<Vidno[]>("spisok"));setAktivnaya(await invoke<string|null>("aktivnaya"));}},
                  {text:"Закрыть…",action:()=>zakrytVkladku(v.id,v.nazvanie)},
                  {text:"Перезагрузить",action:async()=>{const o=await invoke<Otvet>("perezagruzit",{id:v.id});if(!o.ok)setOshibka(o.soobshchenie);setVkladki(await invoke<Vidno[]>("spisok"));}}
                ],e.clientX,e.clientY,setOshibka).catch(err=>setOshibka(String(err)));
              }}
              title={v.nazvanie + (v.privatnaya ? " — приватная супер-вкладка" : "") + " — " + (v.proxy || "прямое соединение") + " · " + cloud.label(v.id,v.privatnaya)}
            >
              <span
                className={"tochka" + (v.zapushchena ? "" : " spit")}
                data-pryamo={v.proxy ? "net" : "da"}
              />
              <span className="imya">{v.nazvanie}</span>{v.privatnaya&&<span className="super-private-badge" title="Приватная супер-вкладка" aria-hidden="true"><svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round"><rect x="3.5" y="7" width="9" height="6.5" rx="1.7"/><path d="M5.5 7V4.8a2.5 2.5 0 0 1 5 0V7"/><path d="M8 9.5v1.5"/></svg></span>}{v.sohranena && <span className="saved-mark" title="Есть в сохранённых"><Icon name="archive" size={13} /></span>}<span className="kratkoe-imya">{Array.from(v.nazvanie).slice(0,3).join('')}</span>
              {!v.privatnaya&&!v.id.startsWith("pvt-")&&<SuperCloudBadge state={cloud.state(v.id)} label={cloud.label(v.id)}/>}
              <span className="krest" title="Закрыть" onClick={(e) => zakrytVkladku(v.id, v.nazvanie, e)}>
                ×
              </span>
            </button>
  );}
  function renderSaved(v:Nastrojki){return <button key={v.id} type="button" data-super-id={v.id} data-cloud-state={cloud.state(v.id)} data-cloud-saved="true" className={"vkladka super-cloud-saved"+(cloud.selected(v.id)?" super-cloud-selected":"")+(superColor(v.cvet)?" super-colored":"")} style={superColor(v.cvet)?{"--super-color":v.cvet} as CSSProperties:undefined} aria-label={v.nazvanie+" — сохранённая супер-вкладка, открыть"} title={v.nazvanie+" · Сохранена на устройстве; открыть · "+cloud.label(v.id)} onClick={()=>void otkrytSohranennuyu(v.id)}>
    <span className="super-cloud-saved-name"><span className="imya">{v.nazvanie}</span><small>Сохранена · открыть</small></span><span className="kratkoe-imya">{Array.from(v.nazvanie).slice(0,3).join('')}</span><SuperCloudBadge state={cloud.state(v.id)} label={cloud.label(v.id)}/>
  </button>;}
  return (
    <div className={"okno" + (nastrojki.panel_svernuta ? " panel-svernuta" : "") + (tyanem ? " tyanem-panel" : "")} style={{ "--reika-w": (nastrojki.panel_svernuta ? 52 : nastrojki.panel_shirina) + "px" } as CSSProperties} onMouseDownCapture={(e) => { if(e.button!==0||editableTarget(e.target))return;void invoke("fokus_obolochke").catch(() => {}); }} onContextMenu={(e) => {if(!editableTarget(e.target))e.preventDefault();}}>
      {/* Рейка супер-вкладок слева — как список серверов в NexusSSH:
          горизонтальная полоса съедала высоту широкоформатников. */}
      <div className="reika">
        <div className="zagolovok"><span className="brand"><img src="/nexusbro.png" width="23" height="23" alt=""/>NexusBro</span><button className="svernut-panel" aria-label={nastrojki.panel_svernuta ? "Развернуть левую панель" : "Свернуть левую панель"}
          onClick={() => { void sohranitNastrojki({ ...nastrojkiRef.current, panel_svernuta: !nastrojkiRef.current.panel_svernuta }).catch((e) => setOshibka(String(e))); }}>{nastrojki.panel_svernuta ? "»" : "«"}</button></div>
        <div className="spisok" onDragOver={(e) => { if (e.dataTransfer.types.includes("application/nexusbro-super")) e.preventDefault(); }} onDrop={(e) => void perenestiSuper(null, e)}>
          {hasCloud&&<section className="super-cloud-group" data-cloud-group="cloud" aria-label="Облако — выбраны для синхронизации">
            <SuperCloudHeading cloud receipt={cloud.lastSync} state={cloud.overall}/>
            {cloudOpen.map(renderSuper)}{cloudClosed.map(renderSaved)}
          </section>}
          <section className="super-cloud-group" data-cloud-group="local" aria-label="Супер-вкладки на этом устройстве"><SuperCloudHeading cloud={false}/>{localOpen.map(renderSuper)}{localClosed.map(renderSaved)}</section>
        </div>
        {!nastrojki.panel_svernuta && <div className="razdelitel-paneli" role="separator" aria-label="Ширина левой панели" aria-orientation="vertical" tabIndex={0}
          aria-valuemin={140} aria-valuemax={400} aria-valuenow={nastrojki.panel_shirina}
          onKeyDown={(e) => { if (e.key === "ArrowLeft" || e.key === "ArrowRight") { e.preventDefault(); void sohranitNastrojki({ ...nastrojkiRef.current, panel_shirina: Math.max(140, Math.min(400, nastrojkiRef.current.panel_shirina + (e.key === "ArrowRight" ? 10 : -10))) }).catch((e) => setOshibka(String(e))); } }}
          onPointerDown={(e) => { if (e.button !== 0) return; e.preventDefault(); e.currentTarget.setPointerCapture(e.pointerId); setTyanem(true); skrytiePriPeretaskivanii.current = invoke("pokazat_dvizhki", { vidno: false }); }}
          onPointerMove={(e) => { if (e.currentTarget.hasPointerCapture(e.pointerId)) panel({ ...nastrojkiRef.current, panel_shirina: Math.round(Math.max(140, Math.min(400, window.innerWidth * .45, e.clientX))) }); }}
          onLostPointerCapture={() => { setTyanem(false); void sohranitNastrojki(nastrojkiRef.current).catch((e) => setOshibka(String(e))); void skrytiePriPeretaskivanii.current.catch(() => {}).then(() => invoke("pokazat_dvizhki", { vidno: true })).catch((e) => setOshibka(String(e))); }}
          onPointerUp={(e) => { if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId); }} />}
        <div className="niz">
          <button className="plyus" aria-label="Новая супер-вкладка" title="Новая супер-вкладка" onClick={() => otkrytDialog()} disabled={zanyat}>
            <Icon name="plus" />
          </button>
          <button
            className="plyus"
            title="Сохранённые супер-вкладки"
            disabled={zanyat}
            onClick={async () => {
              if (menuSohranennyh) {
                await zakrytMenyu();
              } else {
                await invoke("pokazat_dvizhki", { vidno: false });
                setMenuSohranennyh(await invoke<Nastrojki[]>("sohranennye"));
              }
            }}
          >
            <Icon name="archive" />
          </button>
          <button
            className="plyus"
            title="Проверить обновления"
            onClick={async () => {
              setObnovlenie("Проверяю…");
              await proverit_obnovlenie(
                true,
                false,
                (s) => {
                  setObnovlenie(s);
                  setTimeout(() => setObnovlenie(""), 8000);
                },
                (polucheno, vsego) => setProgress({ polucheno, vsego })
              );
              setProgress(null);
              if (obnovlenie === "Проверяю…") setObnovlenie("");
            }}
          >
            <Icon name="reload" />
          </button>
          <button
            className="plyus"
            title="Диагностика"
            onClick={async () => {
              const tekst = await invoke<string>("diagnostika");
              await invoke("pokazat_dvizhki", { vidno: false });
              setDiagnostika(tekst);
            }}
          >
            <Icon name="code" />
          </button>
          <button className="plyus" title="Настройки NexusBro" onClick={async () => {
            await invoke("pokazat_dvizhki", { vidno: false }); setNastroykiOtkryty(true);
          }}><Icon name="settings" /></button>
        </div>
        <Biblioteka superId={aktivnaya} superName={vkladki.find(v=>v.id===aktivnaya)?.nazvanie??aktivnaya??""}/>
        <div className="versiya-app" title={"NexusBro " + (versiya || "…")}>v{versiya || "…"}</div>
      </div>

      <div className="kolonka">
        {/* Тонкая полоса сверху: имя активной супер-вкладки и счётчики.
            Здесь же позже появятся обычные вкладки активной супер-вкладки. */}
        <ObychnyeVkladki panelZakladok={!!nastrojki.panel_zakladok} onToggleBookmarks={()=>{void sohranitNastrojki({...nastrojkiRef.current,panel_zakladok:!nastrojkiRef.current.panel_zakladok}).catch(e=>setOshibka(String(e)));}} poleKontenta={mesto} novayaStranica={nastrojki.novaya_stranica} poisk={nastrojki.poisk} id={aktivnaya} nazvanie={vkladki.find((v) => v.id === aktivnaya)?.nazvanie ?? ""}>
          {obnovlenie && <span className="schet" title="Обновление">{obnovlenie}</span>}
          {aktivnaya && <button type="button" className="schet protection-button" title={"Защита · Заблокировано: "+zarezano} aria-label="Защита супер-вкладки" onClick={() => void otkrytZashchitu()}><Icon name="shield" size={18} /></button>}
        </ObychnyeVkladki>
        {progress && (
          <div className="polosa-progress">
            <div className="progress-liniya">
              <div
                className="progress-zalivka"
                style={{
                  width: progress.vsego > 0 ? `${Math.min(100, (progress.polucheno / progress.vsego) * 100)}%` : "30%",
                }}
              />
            </div>
            <span className="progress-tekst">
              {progress.vsego > 0
                ? `${Math.round((progress.polucheno / progress.vsego) * 100)}%`
                : `${(progress.polucheno / 1048576).toFixed(1)} МБ`}
            </span>
          </div>
        )}

        <div className="pole" ref={mesto}>
          {vkladki.length === 0 && (
            <div className="pusto">
              {dvizhokEst ? (
                <>
                  <h1>NexusBro</h1>
                  <p>
                    Супер-вкладка — это отдельный браузер со своим выходом в сеть.
                    Внутри неё открывай сколько угодно обычных вкладок: весь сайт
                    со всеми его адресами пойдёт через выбранный выход.
                  </p>
                  <button onClick={() => otkrytDialog()}>Создать первую</button>
                </>
              ) : (
                <>
                  <h1>Движок не найден</h1>
                  <p>
                    Файлы движка CEF должны лежать рядом с программой.
                    Переустанови NexusBro.
                  </p>
                </>
              )}
            </div>
          )}
          {oshibka && <div className="oshibka">{oshibka}</div>}
        </div>
      </div>

      {securityOpen&&<SecuritySettings onClose={()=>{setSecurityOpen(false);void invoke("pokazat_dvizhki",{vidno:true});}}/>}
      {nastroykiOtkryty && <NastrojkiOkno value={nastrojki} onSave={sohranitNastrojki} onClose={() => {
        setNastroykiOtkryty(false); void invoke("pokazat_dvizhki", { vidno: true });
      }} />}
      {dialog && (
        <Dialog
          zanyat={zanyat}
          pravka={pravka}
          onOtmena={() => zakrytDialog()}
          onOk={pravka ? pravit : dobavit}
          vyhody={nastrojki.vyhody}
          oshibka={oshibka}
        />
      )}
      {menuSohranennyh !== null && (
        <div className="zatemnenie-prozrachnoe" onClick={() => zakrytMenyu()}>
          <div className="vypadajka" onClick={(e) => e.stopPropagation()}>
            {menuSohranennyh.length === 0 && <div className="pusto-v-menyu">Нет сохранённых</div>}
            {menuSohranennyh.map((n) => (
              <div key={n.id} className="stroka-menyu" onClick={() => otkrytSohranennuyu(n.id)}>
                <span className="tochka" data-pryamo={n.proxy ? "net" : "da"} />
                {n.nazvanie}
                <span
                  className="krest"
                  title="Выбросить насовсем"
                  onClick={(e) => udalitSohranennuyu(n.id, n.nazvanie, e)}
                >
                  ×
                </span>
              </div>
            ))}
          </div>
        </div>
      )}

      {vopros && (
        <div className="zatemnenie" onClick={() => vopros.razreshit(null)}>
          <div className="dialog" onClick={(e) => e.stopPropagation()}>
            <h2>Сохранить вкладку «{vopros.nazvanie}»?</h2>
            <p>
              В её профиле есть входы, история и открытые страницы.
              Сохранённую вкладку можно открыть заново через ▾ у кнопки «+».
            </p>
            <div className="knopki">
              <button onClick={() => vopros.razreshit(null)}>Отмена</button>
              <button onClick={() => vopros.razreshit(false)}>Не сохранять</button>
              <button className="glavnaya" onClick={() => vopros.razreshit(true)}>
                Сохранить
              </button>
            </div>
          </div>
        </div>
      )}
      {zashchita && <Blokirovki journal={zashchita} name={vkladki.find((v) => v.id === aktivnaya)?.nazvanie ?? ""} onClose={() => { setZashchita(null); void invoke("pokazat_dvizhki", { vidno: true }); }} />}
      {diagnostika !== null && (
        <div className="zatemnenie">
          <div className="dialog diagnostika-okno" onClick={(e) => e.stopPropagation()}>
            <h2>Диагностика</h2>
            <pre>{diagnostika}</pre>
            <div className="knopki">
              <button
                onClick={async () => {
                  await navigator.clipboard.writeText(diagnostika).catch(() => {});
                }}
              >
                Скопировать
              </button>
              <button
                className="glavnaya"
                onClick={async () => {
                  setDiagnostika(null);
                  await invoke("pokazat_dvizhki", { vidno: true });
                }}
              >
                Закрыть
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

function Dialog(props: {
  zanyat: boolean;
  pravka: Nastrojki | null;
  vyhody: Vyhod[];
  oshibka: string;
  onOtmena: () => void;
  onOk: (n: string, p: string, a: string, r: boolean, pu: boolean, uzel: string, privatnaya:boolean, cvet:string) => void;
}) {
  const [nazvanie, setNazvanie] = useState(props.pravka?.nazvanie ?? "");
  const [proxy, setProxy] = useState(props.pravka?.proxy ?? "");
  const [uzel, setUzel] = useState(props.pravka?.proxy_uzel ?? "");
  const [uzly, setUzly] = useState<{ imya: string; ssylka: string }[]>([]);
  const [zagruzka, setZagruzka] = useState(false);
  const [oshibkaPodpiski, setOshibkaPodpiski] = useState("");
  const zapros = useRef(0);
  useEffect(() => () => { zapros.current++; }, []);
  async function zagruzitUzly() {
    const nomer = ++zapros.current;
    setZagruzka(true);
    setOshibkaPodpiski("");
    try {
      const list = await invoke<{ imya: string; ssylka: string }[]>("uzly_podpiski", { url: proxy.trim() });
      if (nomer === zapros.current) setUzly(list);
    } catch (e) {
      if (nomer === zapros.current) setOshibkaPodpiski(String(e));
    } finally {
      if (nomer === zapros.current) setZagruzka(false);
    }
  }
  useEffect(() => {
    if (!/^https?:\/\/[^/]+\/.+/.test(proxy.trim())) return;
    const timer = setTimeout(() => { void zagruzitUzly(); }, 400);
    return () => { clearTimeout(timer); zapros.current++; };
  }, [proxy]);
  const [adres, setAdres] = useState(props.pravka?.adres ?? "");
  const [rezat, setRezat] = useState(props.pravka?.rezat_reklamu ?? true);
  const [privatnaya,setPrivatnaya]=useState(props.pravka?.privatnaya??false);
  const [cvet,setCvet]=useState(superColor(props.pravka?.cvet)??"");
  const [pustaya, setPustaya] = useState(props.pravka?.pustaya ?? false);
  return (
    <div className="zatemnenie" onClick={props.zanyat ? undefined : props.onOtmena}>
      <div className="dialog" onClick={(e) => e.stopPropagation()}>
        <h2>{props.pravka ? "Настройки супер-вкладки" : "Новая супер-вкладка"}</h2>
        {props.oshibka && <p className="oshibka-nastroek" role="alert">{props.oshibka}</p>}
        <label>
          Название
          <input
            autoFocus
            value={nazvanie}
            onChange={(e) => setNazvanie(e.target.value)}
            placeholder="Работа"
          />
        </label>
        <fieldset className="super-color-picker" disabled={props.zanyat}>
          <legend>Цвет супер-вкладки</legend>
          <div className="super-color-options">{superColors.map(c=><label key={c.value} title={c.label}>
            <input type="radio" name="super-color" value={c.value} checked={cvet===c.value} onChange={()=>setCvet(c.value)} aria-label={c.label}/>
            <span className={"super-color-swatch"+(c.value?"":" no-color")} style={c.value?{"--swatch-color":c.value} as CSSProperties:undefined}>{!c.value&&<svg viewBox="0 0 18 18" width="18" height="18" aria-hidden="true"><path d="m4 14 10-10" stroke="currentColor" strokeWidth="1.5"/></svg>}</span>
          </label>)}</div>
          <small>Цвет заливки применяется сразу, в том числе в свёрнутой панели. Активная вкладка выделяется отдельно. Замок обозначает приватность.</small>
        </fieldset>
        {props.vyhody.length > 0 && <label>Сохранённое подключение<Vybor aria-label="Сохранённое подключение" value={!proxy ? "" : props.vyhody.some((v) => v.adres === proxy) ? proxy : "manual"} onChange={(e) => {
          if (e.target.value !== "manual") { zapros.current++; setProxy(e.target.value); setUzel(""); setUzly([]); setZagruzka(false); }
        }}><option value="manual" disabled>Адрес введён вручную</option><option value="">Прямое соединение</option>{props.vyhody.map((v) => <option value={v.adres} key={v.id}>{v.nazvanie}</option>)}</Vybor></label>}
        <label>
          Выход в сеть
          <input
            value={proxy}
            onChange={(e) => {
              zapros.current++;
              setProxy(e.target.value); setUzel(""); setUzly([]);
              setZagruzka(false); setOshibkaPodpiski("");
            }}
            placeholder="пусто = напрямую"
          />
          <small>
            Подойдёт ссылка подписки, ссылка на узел <code>vless://…</code>,
            адрес <code>socks5://127.0.0.1:1080</code> или веб-прокси{" "}
            <code>http://адрес:3128</code>. Можно с именем и паролем:{" "}
            <code>socks5://имя:пароль@адрес:порт</code>.
          </small>
        </label>
        {/^https?:\/\/[^/]+\/.+/.test(proxy.trim()) && <>
          <button type="button" className="zagruzit-uzly" disabled={zagruzka || props.zanyat} onClick={() => void zagruzitUzly()}>
            {zagruzka ? "Загружаю узлы…" : "Загрузить узлы подписки"}
          </button>
          <label>Узел подписки
            <Vybor aria-label="Узел подписки" className="uzel-podpiski" value={uzel} onChange={(e) => setUzel(e.target.value)} disabled={props.zanyat}>
              <option value="">Первый узел подписки</option>
              {uzel && !uzly.some((u) => u.ssylka === uzel) && <option value={uzel}>Сохранённый узел (обнови список для проверки)</option>}
              {uzly.map((u, i) => <option key={i} value={u.ssylka}>{u.imya}</option>)}
            </Vybor>
          </label>
          {oshibkaPodpiski && <small role="alert">{oshibkaPodpiski}</small>}
        </>}
        <label>
          Открыть сразу
          <input
            value={adres}
            onChange={(e) => setAdres(e.target.value)}
            placeholder="about:blank"
          />
        </label>
        <label className="galka">
          <input
            type="checkbox"
            checked={rezat}
            onChange={(e) => setRezat(e.target.checked)}
          />
          <span>
            Резать рекламу
            <small>
              Работает во всех вкладках сразу, без расширений и без своего
              сертификата в системе.
            </small>
          </span>
        </label>
        <label className="galka">
          <input
            type="checkbox"
            checked={pustaya}
            onChange={(e) => setPustaya(e.target.checked)}
          />
          <span>
            Открывать пустой
            <small>
              Не восстанавливать открытые вкладки с прошлого раза — начинать
              со страницы «Открыть сразу».
            </small>
          </span>
        </label>
        <label className="galka"><input type="checkbox" disabled={!!props.pravka||props.zanyat} checked={privatnaya} onChange={e=>setPrivatnaya(e.target.checked)}/><span>Приватная супер-вкладка<small>Настройки супер-вкладки сохраняются. История, cookies и данные страниц — только до закрытия сеанса. Скачанные файлы остаются.</small></span></label>
        <div className="knopki">
          <button disabled={props.zanyat} onClick={props.onOtmena}>Отмена</button>
          <button
            className="glavnaya"
            disabled={props.zanyat || !nazvanie.trim()}
            onClick={() => props.onOk(nazvanie.trim(), proxy.trim(), adres.trim(), rezat, pustaya, uzel, privatnaya, cvet)}
          >
            {props.zanyat
              ? props.pravka
                ? "Проверяю и применяю…"
                : "Запускаю…"
              : props.pravka
                ? "Сохранить"
                : "Создать"}
          </button>
        </div>
      </div>
    </div>
  );
}
