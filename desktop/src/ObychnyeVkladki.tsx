import { ReactNode, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type Vkladka = { id: number; adres: string; zagolovok: string; zakreplena: boolean; zagruzka: boolean; nazad: boolean; vpered: boolean };
type Sessiya = { vkladki: Vkladka[]; aktivnaya: number };
type Komanda = { deystvie: string; id?: number; adres?: string; zakreplena?: boolean };

export default function ObychnyeVkladki({ id, nazvanie, children }: { id: string | null; nazvanie: string; children: ReactNode }) {
  const [sessiya, setSessiya] = useState<Sessiya | null>(null);
  const [adres, setAdres] = useState("");
  const [pravka, setPravka] = useState(false);
  const [oshibka, setOshibka] = useState("");
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
        onClick={() => void poslat({ deystvie: "novaya", adres: "about:blank" })}>+</button>
      {children}
    </div>
    <form className="navigaciya" onSubmit={(e) => {
      e.preventDefault();
      if (!aktivnaya) return;
      const text = (pole.current?.value ?? adres).trim();
      const url = !text ? "about:blank" : /^[a-z][a-z0-9+.-]*:/i.test(text) ? text : "https://" + text;
      void poslat({ deystvie: "pereyti", id: aktivnaya.id, adres: url });
      pole.current?.blur();
    }}>
      <button type="button" title="Назад" disabled={!aktivnaya?.nazad} onClick={() => void poslat({ deystvie: "nazad", id: aktivnaya?.id })}>←</button>
      <button type="button" title="Вперёд" disabled={!aktivnaya?.vpered} onClick={() => void poslat({ deystvie: "vpered", id: aktivnaya?.id })}>→</button>
      <button type="button" title="Обновить страницу" disabled={!aktivnaya} onClick={() => void poslat({ deystvie: "obnovit", id: aktivnaya?.id })}>⟳</button>
      <input ref={pole} aria-label="Адрес страницы" placeholder="Введите адрес" disabled={!aktivnaya}
        value={adres} onFocus={() => setPravka(true)} onBlur={() => setPravka(false)} onChange={(e) => setAdres(e.target.value)} />
      {oshibka && <span className="oshibka-navigacii" role="alert" title={oshibka}>{oshibka}</span>}
    </form>
  </>;
}
