import { CSSProperties, useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import NastrojkiOkno, { NastrojkiApp, poUmolchaniyu } from "./Nastrojki";
import ObychnyeVkladki from "./ObychnyeVkladki";
import { proverit_obnovlenie } from "./updater";

type Nastrojki = {
  id: string;
  nazvanie: string;
  proxy: string;
  adres: string;
  rezat_reklamu: boolean;
  pustaya: boolean;
};
type Vidno = Nastrojki & { zapushchena: boolean };
type Otvet = { ok: boolean; soobshchenie: string };

export default function App() {
  const [nastrojki, setNastrojki] = useState<NastrojkiApp>(poUmolchaniyu);
  const nastrojkiRef = useRef(nastrojki);
  const [nastroykiOtkryty, setNastroykiOtkryty] = useState(false);
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
    void invoke<NastrojkiApp>("nastrojki_poluchit").then((n) => {
      panel(n);
      if (n.avtoobnovleniya) void proverit_obnovlenie();
    }).catch((e) => setOshibka(String(e)));
  }, []);
  useEffect(() => { document.documentElement.dataset.tema = nastrojki.tema; }, [nastrojki.tema]);
  const [vkladki, setVkladki] = useState<Vidno[]>([]);
  const [aktivnaya, setAktivnaya] = useState<string | null>(null);
  const aktivnayaRef = useRef<string | null>(null);
  const [dvizhokEst, setDvizhokEst] = useState(true);
  const [dialog, setDialog] = useState(false);
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
  const [menyu, setMenyu] = useState<{ x: number; y: number; vkladka: Vidno } | null>(null);
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
      setVkladki(await invoke<Vidno[]>("spisok"));
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

  async function dobavit(nazvanie: string, proxy: string, adres: string, rezat: boolean, pustaya: boolean) {
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
          pustaya,
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
    setMenyu(null);
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
    await invoke("pokazat_dvizhki", { vidno: false });
    setPravka(n);
    setDialog(true);
  }

  async function pravit(nazvanie: string, proxy: string, adres: string, rezat: boolean, pustaya: boolean) {
    if (!pravka) return;
    setZanyat(true);
    setOshibka("");
    try {
      const o = await invoke<Otvet>("izmenit", {
        nastrojki: { ...pravka, nazvanie, proxy, adres: adres || "about:blank", rezat_reklamu: rezat, pustaya },
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

  async function zakrytVkladku(id: string, nazvanie: string, e?: React.MouseEvent) {
    e?.stopPropagation();
    // Вкладка с данными (входы, история) спрашивает: сохранить или нет.
    // Свежая закрывается сразу, без вопросов.
    let sohranit = false;
    if (await invoke<boolean>("nuzhno_sohranenie", { id })) {
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
    if (!window.confirm(`Выбросить сохранённую вкладку «${nazvanie}» насовсем?\n\nПрофиль со всеми входами будет стёрт.`)) {
      return;
    }
    await invoke<Otvet>("udalit_sohranennuyu", { id });
    setMenuSohranennyh(await invoke<Nastrojki[]>("sohranennye"));
  }

  useEffect(() => {
    aktivnayaRef.current = aktivnaya;
  }, [aktivnaya]);

  return (
    <div className={"okno" + (nastrojki.panel_svernuta ? " panel-svernuta" : "") + (tyanem ? " tyanem-panel" : "")} style={{ "--reika-w": (nastrojki.panel_svernuta ? 52 : nastrojki.panel_shirina) + "px" } as CSSProperties} onMouseDownCapture={() => { void invoke("fokus_obolochke").catch(() => {}); }} onContextMenu={(e) => e.preventDefault()}>
      {/* Рейка супер-вкладок слева — как список серверов в NexusSSH:
          горизонтальная полоса съедала высоту широкоформатников. */}
      <div className="reika">
        <div className="zagolovok"><span>NexusBro</span><button className="svernut-panel" aria-label={nastrojki.panel_svernuta ? "Развернуть левую панель" : "Свернуть левую панель"}
          onClick={() => { void sohranitNastrojki({ ...nastrojkiRef.current, panel_svernuta: !nastrojkiRef.current.panel_svernuta }).catch((e) => setOshibka(String(e))); }}>{nastrojki.panel_svernuta ? "»" : "«"}</button></div>
        <div className="spisok">
          {vkladki.map((v) => (
            <button
              key={v.id}
              className={"vkladka" + (v.id === aktivnaya ? " aktivna" : "")}
              onClick={() => pereklyuchit(v.id)}
              onContextMenu={async (e) => {
                e.preventDefault();
                e.stopPropagation();
                const poziciya = { x: e.clientX, y: e.clientY, vkladka: v };
                await invoke("pokazat_dvizhki", { vidno: false });
                setMenyu(poziciya);
              }}
              title={v.nazvanie + " — " + (v.proxy || "прямое соединение")}
            >
              <span
                className={"tochka" + (v.zapushchena ? "" : " spit")}
                data-pryamo={v.proxy ? "net" : "da"}
              />
              <span className="imya">{v.nazvanie}</span><span className="kratkoe-imya">{v.nazvanie.slice(0, 1)}</span>
              <span className="krest" title="Закрыть" onClick={(e) => zakrytVkladku(v.id, v.nazvanie, e)}>
                ×
              </span>
            </button>
          ))}
        </div>
        {!nastrojki.panel_svernuta && <div className="razdelitel-paneli" role="separator" aria-label="Ширина левой панели" aria-orientation="vertical" tabIndex={0}
          aria-valuemin={140} aria-valuemax={400} aria-valuenow={nastrojki.panel_shirina}
          onKeyDown={(e) => { if (e.key === "ArrowLeft" || e.key === "ArrowRight") { e.preventDefault(); void sohranitNastrojki({ ...nastrojkiRef.current, panel_shirina: Math.max(140, Math.min(400, nastrojkiRef.current.panel_shirina + (e.key === "ArrowRight" ? 10 : -10))) }).catch((e) => setOshibka(String(e))); } }}
          onPointerDown={(e) => { if (e.button !== 0) return; e.preventDefault(); e.currentTarget.setPointerCapture(e.pointerId); setTyanem(true); skrytiePriPeretaskivanii.current = invoke("pokazat_dvizhki", { vidno: false }); }}
          onPointerMove={(e) => { if (e.currentTarget.hasPointerCapture(e.pointerId)) panel({ ...nastrojkiRef.current, panel_shirina: Math.round(Math.max(140, Math.min(400, window.innerWidth * .45, e.clientX))) }); }}
          onLostPointerCapture={() => { setTyanem(false); void sohranitNastrojki(nastrojkiRef.current).catch((e) => setOshibka(String(e))); void skrytiePriPeretaskivanii.current.catch(() => {}).then(() => invoke("pokazat_dvizhki", { vidno: true })).catch((e) => setOshibka(String(e))); }}
          onPointerUp={(e) => { if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId); }} />}
        <div className="niz">
          <button className="plyus" onClick={() => otkrytDialog()} disabled={zanyat}>
            +
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
            ▾
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
            ⟳
          </button>
          <button
            className="plyus"
            title="Диагностика"
            onClick={async () => {
              await invoke("pokazat_dvizhki", { vidno: false });
              const tekst = await invoke<string>("diagnostika");
              setDiagnostika(tekst);
            }}
          >
            🛠
          </button>
          <button className="plyus" title="Настройки NexusBro" onClick={async () => {
            await invoke("pokazat_dvizhki", { vidno: false }); setNastroykiOtkryty(true);
          }}>⚙</button>
        </div>
      </div>

      <div className="kolonka">
        {/* Тонкая полоса сверху: имя активной супер-вкладки и счётчики.
            Здесь же позже появятся обычные вкладки активной супер-вкладки. */}
        <ObychnyeVkladki novayaStranica={nastrojki.novaya_stranica} poisk={nastrojki.poisk} id={aktivnaya} nazvanie={vkladki.find((v) => v.id === aktivnaya)?.nazvanie ?? ""}>
          {obnovlenie && <span className="schet" title="Обновление">{obnovlenie}</span>}
          {zarezano > 0 && (
            <span className="schet" title="Зарезано рекламы в этой вкладке">
              ⛔ {zarezano}
            </span>
          )}
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

      {nastroykiOtkryty && <NastrojkiOkno value={nastrojki} onSave={sohranitNastrojki} onClose={() => {
        setNastroykiOtkryty(false); void invoke("pokazat_dvizhki", { vidno: true });
      }} />}
      {dialog && (
        <Dialog
          zanyat={zanyat}
          pravka={pravka}
          onOtmena={() => zakrytDialog()}
          onOk={pravka ? pravit : dobavit}
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
      {menyu && (
        <div
          className="zatemnenie-prozrachnoe"
          onClick={() => zakrytMenyu()}
          onContextMenu={(e) => {
            e.preventDefault();
            void zakrytMenyu();
          }}
        >
          <div
            className="vypadajka"
            style={{ left: menyu.x, top: menyu.y, right: "auto" }}
            onClick={(e) => e.stopPropagation()}
          >
            <div
              className="stroka-menyu"
              onClick={async () => {
                const v = menyu.vkladka;
                setMenyu(null);
                await nachatPravku(v);
              }}
            >
              Настройки
            </div>
            <div
              className="stroka-menyu"
              onClick={async () => {
                const v = menyu.vkladka;
                setMenyu(null);
                try {
                  await invoke<Otvet>("zakryt", { id: v.id, sohranit: true });
                  setVkladki(await invoke<Vidno[]>("spisok"));
                  setAktivnaya(await invoke<string | null>("aktivnaya"));
                } finally {
                  await invoke("pokazat_dvizhki", { vidno: true });
                }
              }}
            >
              Сохранить и закрыть
            </div>
            <div
              className="stroka-menyu"
              onClick={async () => {
                const v = menyu.vkladka;
                setMenyu(null);
                try {
                  await zakrytVkladku(v.id, v.nazvanie);
                } finally {
                  await invoke("pokazat_dvizhki", { vidno: true });
                }
              }}
            >
              Закрыть…
            </div>
            <div
              className="stroka-menyu"
              onClick={async () => {
                const v = menyu.vkladka;
                setMenyu(null);
                try {
                  const o = await invoke<Otvet>("perezagruzit", { id: v.id });
                  if (!o.ok) setOshibka(o.soobshchenie);
                  setVkladki(await invoke<Vidno[]>("spisok"));
                } finally {
                  await invoke("pokazat_dvizhki", { vidno: true });
                }
              }}
            >
              Перезагрузить
            </div>
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
  onOtmena: () => void;
  onOk: (n: string, p: string, a: string, r: boolean, pu: boolean) => void;
}) {
  const [nazvanie, setNazvanie] = useState(props.pravka?.nazvanie ?? "");
  const [proxy, setProxy] = useState(props.pravka?.proxy ?? "");
  const [adres, setAdres] = useState(props.pravka?.adres ?? "");
  const [rezat, setRezat] = useState(props.pravka?.rezat_reklamu ?? true);
  const [pustaya, setPustaya] = useState(props.pravka?.pustaya ?? false);
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
        <div className="knopki">
          <button onClick={props.onOtmena}>Отмена</button>
          <button
            className="glavnaya"
            disabled={props.zanyat || !nazvanie.trim()}
            onClick={() => props.onOk(nazvanie.trim(), proxy.trim(), adres.trim(), rezat, pustaya)}
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
