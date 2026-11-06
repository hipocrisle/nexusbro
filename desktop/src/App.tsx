import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type Nastrojki = { id: string; nazvanie: string; proxy: string; adres: string };
type Vidno = Nastrojki & { zapushchena: boolean };
type Otvet = { ok: boolean; soobshchenie: string };

export default function App() {
  const [vkladki, setVkladki] = useState<Vidno[]>([]);
  const [aktivnaya, setAktivnaya] = useState<string | null>(null);
  const [dvizhokEst, setDvizhokEst] = useState(true);
  const [dialog, setDialog] = useState(false);
  const [oshibka, setOshibka] = useState("");
  const [zanyat, setZanyat] = useState(false);
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
    invoke<string | null>("aktivnaya").then(setAktivnaya);
    soobshchitMesto();
    window.addEventListener("resize", soobshchitMesto);
    // ядро само сообщает, когда список изменился
    const otpiska = listen<string>("vkladki-izmenilis", async (e) => {
      setVkladki(await invoke<Vidno[]>("spisok"));
      if (e.payload) setAktivnaya(e.payload);
      soobshchitMesto();
    });
    return () => {
      window.removeEventListener("resize", soobshchitMesto);
      otpiska.then((f) => f());
    };
  }, [soobshchitMesto]);

  async function dobavit(nazvanie: string, proxy: string, adres: string) {
    setZanyat(true);
    setOshibka("");
    const id = "sv" + Date.now().toString(36);
    const o = await invoke<Otvet>("otkryt", {
      nastrojki: { id, nazvanie, proxy, adres: adres || "about:blank" },
    });
    setZanyat(false);
    if (!o.ok) {
      setOshibka(o.soobshchenie);
      return;
    }
    setVkladki(await invoke<Vidno[]>("spisok"));
    setAktivnaya(id);
    setDialog(false);
    setTimeout(soobshchitMesto, 100);
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

  async function ostanovit(id: string, e: React.MouseEvent) {
    e.stopPropagation();
    await invoke<Otvet>("ostanovit", { id });
    setVkladki(await invoke<Vidno[]>("spisok"));
  }

  async function udalit(id: string, nazvanie: string, e: React.MouseEvent) {
    e.stopPropagation();
    // ⛔ В профиле вкладки лежат входы в почту и прочее — спрашиваем.
    const ok = window.confirm(
      "Удалить супер-вкладку «" + nazvanie + "»?\n\n" +
        "Вместе с ней пропадёт её профиль: входы на сайты, закладки и история."
    );
    if (!ok) return;
    await invoke<Otvet>("udalit", { id });
    const spisok = await invoke<Vidno[]>("spisok");
    setVkladki(spisok);
    if (aktivnaya === id) setAktivnaya(spisok[0]?.id ?? null);
  }

  return (
    <div className="okno">
      <div className="polosa">
        {vkladki.map((v) => (
          <button
            key={v.id}
            className={"vkladka" + (v.id === aktivnaya ? " aktivna" : "")}
            onClick={() => pereklyuchit(v.id)}
            title={v.proxy || "прямое соединение"}
          >
            <span
              className={"tochka" + (v.zapushchena ? "" : " spit")}
              data-pryamo={v.proxy ? "net" : "da"}
            />
            {v.nazvanie}
            {v.zapushchena && (
              <span
                className="stop"
                title="Остановить, не удаляя"
                onClick={(e) => ostanovit(v.id, e)}
              >
                ■
              </span>
            )}
            <span className="krest" title="Удалить" onClick={(e) => udalit(v.id, v.nazvanie, e)}>
              ×
            </span>
          </button>
        ))}
        <button className="plyus" onClick={() => setDialog(true)} disabled={zanyat}>
          +
        </button>
      </div>

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
                <button onClick={() => setDialog(true)}>Создать первую</button>
              </>
            ) : (
              <>
                <h1>Движок не найден</h1>
                <p>
                  Положи ungoogled-chromium в папку <code>chromium</code> рядом с
                  программой — или установи его в систему.
                </p>
              </>
            )}
          </div>
        )}
        {oshibka && <div className="oshibka">{oshibka}</div>}
      </div>

      {dialog && <Dialog zanyat={zanyat} onOtmena={() => setDialog(false)} onOk={dobavit} />}
    </div>
  );
}

function Dialog(props: {
  zanyat: boolean;
  onOtmena: () => void;
  onOk: (n: string, p: string, a: string) => void;
}) {
  const [nazvanie, setNazvanie] = useState("");
  const [proxy, setProxy] = useState("");
  const [adres, setAdres] = useState("");
  return (
    <div className="zatemnenie" onClick={props.onOtmena}>
      <div className="dialog" onClick={(e) => e.stopPropagation()}>
        <h2>Новая супер-вкладка</h2>
        <label>
          Название
          <input
            autoFocus
            value={nazvanie}
            onChange={(e) => setNazvanie(e.target.value)}
            placeholder="Работа"
          />
        </label>
        <label>
          Выход в сеть
          <input
            value={proxy}
            onChange={(e) => setProxy(e.target.value)}
            placeholder="пусто = напрямую, или socks5://127.0.0.1:1080"
          />
        </label>
        <label>
          Открыть сразу
          <input
            value={adres}
            onChange={(e) => setAdres(e.target.value)}
            placeholder="about:blank"
          />
        </label>
        <div className="knopki">
          <button onClick={props.onOtmena}>Отмена</button>
          <button
            className="glavnaya"
            disabled={props.zanyat || !nazvanie.trim()}
            onClick={() => props.onOk(nazvanie.trim(), proxy.trim(), adres.trim())}
          >
            {props.zanyat ? "Запускаю…" : "Создать"}
          </button>
        </div>
      </div>
    </div>
  );
}
