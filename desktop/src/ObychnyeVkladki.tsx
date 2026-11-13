import { ReactNode, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type Vkladka = { id: number; adres: string; zagolovok: string; zakreplena: boolean; zagruzka: boolean; nazad: boolean; vpered: boolean };
type Sessiya = { vkladki: Vkladka[]; aktivnaya: number };
type Komanda = { deystvie: string; id?: number; adres?: string; zakreplena?: boolean; sprava?: boolean };

export default function ObychnyeVkladki({ id, nazvanie, children, novayaStranica, poisk }: { id: string | null; nazvanie: string; children: ReactNode; novayaStranica: string; poisk: string }) {
  const [sessiya, setSessiya] = useState<Sessiya | null>(null);
  const [adres, setAdres] = useState("");
  const [pravka, setPravka] = useState(false);
  const [oshibka, setOshibka] = useState("");
  const [menu, setMenu] = useState<{ x: number; y: number; v: Vkladka } | null>(null);
  const vybratPriPusteni = useRef(false);
  const pole = useRef<HTMLInputElement>(null);
  useEffect(() => {
    setSessiya(null);
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
  return <>
    <div className="polosa">
      <span className="aktivnaya-imya">{nazvanie}</span>
      <div className="obychnye" role="tablist" aria-label="Обычные вкладки">
        {sessiya?.vkladki.map((v) => <div key={v.id} role="tab" aria-selected={v.id === sessiya.aktivnaya} tabIndex={0}
          data-id={v.id} className={"obychnaya" + (v.id === sessiya.aktivnaya ? " vybrana" : "") + (v.zakreplena ? " zakreplena" : "")}
          onAuxClick={(e) => { if (e.button === 1) { e.preventDefault(); void poslat({ deystvie: "zakryt", id: v.id }); } }}
          onContextMenu={async (e) => { e.preventDefault(); e.stopPropagation(); const m = { x: Math.max(8, Math.min(e.clientX, window.innerWidth - 260)), y: Math.max(8, Math.min(e.clientY, window.innerHeight - 350)), v }; await invoke("pokazat_dvizhki", { vidno: false }); setMenu(m); }}
          title={v.zagolovok + "\n" + v.adres} onClick={() => void poslat({ deystvie: "vybrat", id: v.id })}
          onKeyDown={(e) => { if (e.target !== e.currentTarget) return; if (e.key === "Enter" || e.key === " ") { e.preventDefault(); void poslat({ deystvie: "vybrat", id: v.id }); } }}>
          <span className="obychnaya-imya">{v.zagruzka ? "◌ " : ""}{v.zakreplena ? (v.zagolovok || "Н").slice(0, 1) : v.zagolovok || "Новая вкладка"}</span>
          <button className="zakrepit" title={v.zakreplena ? "Открепить вкладку" : "Закрепить вкладку"}
            onClick={(e) => { e.stopPropagation(); void poslat({ deystvie: "zakrepit", id: v.id, zakreplena: !v.zakreplena }); }}>⌖</button>
          <button className="zakryt-obychnuyu" title="Закрыть обычную вкладку"
            onClick={(e) => { e.stopPropagation(); void poslat({ deystvie: "zakryt", id: v.id }); }}>×</button>
        </div>)}
      </div>
      <button className="novaya-obychnaya" title="Новая обычная вкладка" disabled={!id}
        onClick={() => void poslat({ deystvie: "novaya", adres: novayaStranica })}>+</button>
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
      <button type="button" title="Назад" disabled={!aktivnaya?.nazad} onClick={() => void poslat({ deystvie: "nazad", id: aktivnaya?.id })}>←</button>
      <button type="button" title="Вперёд" disabled={!aktivnaya?.vpered} onClick={() => void poslat({ deystvie: "vpered", id: aktivnaya?.id })}>→</button>
      <button type="button" title="Обновить страницу" disabled={!aktivnaya} onClick={() => void poslat({ deystvie: "obnovit", id: aktivnaya?.id })}>⟳</button>
      <input ref={pole} aria-label="Адрес страницы" placeholder="Введите адрес" disabled={!aktivnaya}
        value={adres} onFocus={(e) => { setPravka(true); e.currentTarget.select(); vybratPriPusteni.current = true; }} onMouseUp={(e) => { if (vybratPriPusteni.current) { e.preventDefault(); vybratPriPusteni.current = false; } }} onBlur={() => setPravka(false)} onChange={(e) => setAdres(e.target.value)} />
      {oshibka && <span className="oshibka-navigacii" role="alert" title={oshibka}>{oshibka}</span>}
    </form>
    {menu && <div className="zatemnenie-prozrachnoe" onClick={() => void zakrytMenu()} onContextMenu={(e) => { e.preventDefault(); void zakrytMenu(); }}>
      <div className="vypadajka menu-obychnyh" role="menu" aria-label="Действия с вкладкой" style={{ left: menu.x, top: menu.y, bottom: "auto" }} onClick={(e) => e.stopPropagation()}>
        <button role="menuitem" onClick={() => void zakrytMenu({ deystvie: "novaya", adres: novayaStranica })}>Новая вкладка</button>
        <button role="menuitem" onClick={() => void zakrytMenu({ deystvie: "obnovit", id: menu.v.id })}>Обновить</button>
        <button role="menuitem" onClick={() => void zakrytMenu({ deystvie: "novaya", adres: menu.v.adres })}>Дублировать</button>
        <button role="menuitem" onClick={() => void zakrytMenu({ deystvie: "zakrepit", id: menu.v.id, zakreplena: !menu.v.zakreplena })}>{menu.v.zakreplena ? "Открепить" : "Закрепить"}</button>
        <hr />
        <button role="menuitem" onClick={() => void zakrytMenu({ deystvie: "zakryt", id: menu.v.id })}>Закрыть вкладку</button>
        <button role="menuitem" disabled={!sessiya?.vkladki.some((v) => v.id !== menu.v.id && !v.zakreplena)} onClick={() => void zakrytMenu({ deystvie: "zakryt_drugie", id: menu.v.id, sprava: false })}>Закрыть другие вкладки</button>
        <button role="menuitem" disabled={!sessiya?.vkladki.slice(sessiya.vkladki.findIndex((v) => v.id === menu.v.id) + 1).some((v) => !v.zakreplena)} onClick={() => void zakrytMenu({ deystvie: "zakryt_drugie", id: menu.v.id, sprava: true })}>Закрыть вкладки справа</button>
      </div>
    </div>}
  </>;
}
