import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { proverit_obnovlenie } from "./updater";

type Nastrojki = { id: string; nazvanie: string; proxy: string; adres: string; rezat_reklamu: boolean };
type Vidno = Nastrojki & { zapushchena: boolean };
type Otvet = { ok: boolean; soobshchenie: string };

export default function App() {
  const [vkladki, setVkladki] = useState<Vidno[]>([]);
  const [aktivnaya, setAktivnaya] = useState<string | null>(null);
  const aktivnayaRef = useRef<string | null>(null);
  const [dvizhokEst, setDvizhokEst] = useState(true);
  const [dialog, setDialog] = useState(false);
  const [pravka, setPravka] = useState<Nastrojki | null>(null);
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
    invoke<string | null>("aktivnaya").then(setAktivnaya);
    soobshchitMesto();
    proverit_obnovlenie();
    window.addEventListener("resize", soobshchitMesto);
    // ядро само сообщает, когда список изменился
    const otpiska = listen<string>("vkladki-izmenilis", async (e) => {
      setVkladki(await invoke<Vidno[]>("spisok"));
      if (e.payload) setAktivnaya(e.payload);
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
      otpiska.then((f) => f());
      clearInterval(chasy);
    };
  }, [soobshchitMesto]);

  async function dobavit(nazvanie: string, proxy: string, adres: string, rezat: boolean) {
    setZanyat(true);
    setOshibka("");
    const id = "sv" + Date.now().toString(36);
    try {
      const o = await invoke<Otvet>("otkryt", {
        nastrojki: {
          id,
          nazvanie,
          proxy,
          adres: adres || "about:blank",
          rezat_reklamu: rezat,
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
    await invoke("pokazat_dvizhki", { vidno: false });
    setDialog(true);
  }
  async function zakrytDialog() {
    setDialog(false);
    setPravka(null);
    await invoke("pokazat_dvizhki", { vidno: true });
  }

  // Правка супер-вкладки: тот же диалог, но с её настройками.
  async function nachatPravku(n: Nastrojki) {
    await invoke("pokazat_dvizhki", { vidno: false });
    setPravka(n);
    setDialog(true);
  }

  async function pravit(nazvanie: string, proxy: string, adres: string, rezat: boolean) {
    if (!pravka) return;
    setZanyat(true);
    setOshibka("");
    try {
      const o = await invoke<Otvet>("izmenit", {
        nastrojki: { ...pravka, nazvanie, proxy, adres: adres || "about:blank", rezat_reklamu: rezat },
      });
      if (!o.ok) {
        setOshibka(o.soobshchenie);
        return;
      }
      setVkladki(await invoke<Vidno[]>("spisok"));
      setTimeout(soobshchitMesto, 100);
    } finally {
      setZanyat(false);
      setDialog(false);
      setPravka(null);
      await invoke("pokazat_dvizhki", { vidno: true });
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

  useEffect(() => {
    aktivnayaRef.current = aktivnaya;
  }, [aktivnaya]);

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
            <span
              className="pravka"
              title="Настройки супер-вкладки"
              onClick={(e) => {
                e.stopPropagation();
                nachatPravku(v);
              }}
            >
              ✎
            </span>
            <span className="krest" title="Удалить" onClick={(e) => udalit(v.id, v.nazvanie, e)}>
              ×
            </span>
          </button>
        ))}
        <button className="plyus" onClick={() => otkrytDialog()} disabled={zanyat}>
          +
        </button>
        {zarezano > 0 && (
          <span className="schet" title="Зарезано рекламы в этой вкладке">
            ⛔ {zarezano}
          </span>
        )}
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
                <button onClick={() => otkrytDialog()}>Создать первую</button>
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

      {dialog && (
        <Dialog
          zanyat={zanyat}
          pravka={pravka}
          onOtmena={() => zakrytDialog()}
          onOk={pravka ? pravit : dobavit}
        />
      )}
    </div>
  );
}

function Dialog(props: {
  zanyat: boolean;
  pravka: Nastrojki | null;
  onOtmena: () => void;
  onOk: (n: string, p: string, a: string, r: boolean) => void;
}) {
  const [nazvanie, setNazvanie] = useState(props.pravka?.nazvanie ?? "");
  const [proxy, setProxy] = useState(props.pravka?.proxy ?? "");
  const [adres, setAdres] = useState(props.pravka?.adres ?? "");
  const [rezat, setRezat] = useState(props.pravka?.rezat_reklamu ?? true);
  return (
    <div className="zatemnenie" onClick={props.onOtmena}>
      <div className="dialog" onClick={(e) => e.stopPropagation()}>
        <h2>{props.pravka ? "Настройки супер-вкладки" : "Новая супер-вкладка"}</h2>
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
            placeholder="пусто = напрямую"
          />
          <small>
            Подойдёт ссылка подписки, ссылка на узел <code>vless://…</code>,
            адрес <code>socks5://127.0.0.1:1080</code> или веб-прокси{" "}
            <code>http://адрес:3128</code>. Можно с именем и паролем:{" "}
            <code>socks5://имя:пароль@адрес:порт</code>.
          </small>
        </label>
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
        <div className="knopki">
          <button onClick={props.onOtmena}>Отмена</button>
          <button
            className="glavnaya"
            disabled={props.zanyat || !nazvanie.trim()}
            onClick={() => props.onOk(nazvanie.trim(), proxy.trim(), adres.trim(), rezat)}
          >
            {props.zanyat
              ? props.pravka
                ? "Перезапускаю…"
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
