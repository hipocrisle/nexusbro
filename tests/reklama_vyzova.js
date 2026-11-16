// Проверка кода обрезки вызова из `desktop/src-tauri/src/cef_vkladki.rs`.
//
// Зачем отдельный файл: скрипт живёт в странице, а не в Rust. Компиляция его
// не проверит, поэтому берём ровно те строки, что собираются в Rust, и гоняем
// на маленьком моке DOM. Запуск: `node tests/reklama_vyzova.js`.
const fs = require("fs");
const path = require("path");
const vm = require("vm");

const rust = fs.readFileSync(
  path.join(__dirname, "..", "desktop/src-tauri/src/cef_vkladki.rs"),
  "utf8",
);
const nachalo = rust.indexOf("const SKRIPT_VYZOVA_ZAGOTOVKA: &str = concat!(");
if (nachalo < 0) throw new Error("не нашёл заготовку скрипта в cef_vkladki.rs");
const hvost = rust.slice(nachalo).split("\n);")[0];
const kuski = [...hvost.matchAll(/r#"(.*?)"#/gs)].map((m) => m[1]);
if (!kuski.length) throw new Error("не разобрал строки скрипта");

const PRAVILA = ["/promo/p.gif", "/ads/banner", "/click?", "yclid=", "adunit="];
const kod = kuski.join("").replace("__PRAVILA__", JSON.stringify(PRAVILA));

const skript = kuski.join("").replace("__PRAVILA__", JSON.stringify(PRAVILA));

// ── мок страницы ──
function sdelatStranicu(adres) {
  const uzel = () => ({ nodeType: 1, tagName: "", __nbOk: false, attrs: {}, observers: [] });
  const observers = [];
  const otpravleno = [];
  const window = {
    // fetch-мок: запоминаем то, что реально ушло
    fetch(u) {
      otpravleno.push(String(u));
      return Promise.resolve({ ok: true });
    },
    XMLHttpRequest: class {
      open(m, u) {
        this.u = u;
      }
      send() {
        otpravleno.push(`xhr:${this.u}`);
      }
    },
    MutationObserver: class {
      constructor(fn) {
        this.fn = fn;
        this.uzly = [];
        observers.push(this);
      }
      observe(el, opts) {
        this.uzly.push({ el, opts });
      }
    },
  };
  const document = {
    documentElement: uzel(),
    createElement(tag) {
      const el = uzel();
      el.tagName = tag.toUpperCase();
      el.src = "";
      el.href = "";
      el.setAttribute = (k, v) => (el[k] = v);
      el.removeAttribute = (k) => (el[k] = "");
      return el;
    },
  };
  const location = new URL(adres);
  return { window, document, location, observers, otpravleno };
}

async function progon() {
  const oshibki = [];
  const proverit = (uslovie, chto) => {
    if (!uslovie) oshibki.push(chto);
  };

  const s = sdelatStranicu("https://site.ru/news/2026");
  s.window.Promise = Promise;
  const g1 = vm.createContext(s.window);
  s.window.window = g1;
  g1.document = s.document;
  g1.location = s.location;
  g1.MutationObserver = s.window.MutationObserver;
  vm.runInContext(skript, g1);

  // fetch: рекламный адрес отменяется, обычный уходит
  let ostanovleno = false;
  try {
    await s.window.fetch("https://site.ru/promo/p.gif");
  } catch (e) {
    ostanovleno = true;
  }
  proverit(ostanovleno, "fetch рекламного адреса не отменён");
  await s.window.fetch("https://site.ru/news/2026?text=погода");
  proverit(s.otpravleno.some((u) => u.includes("text=погода")), "обычный fetch не ушёл");

  // XHR: рекламный не открывается, обычный уходит
  const x1 = new s.window.XMLHttpRequest();
  x1.open("GET", "https://site.ru/ads/banner.jpg?adunit=1");
  x1.send();
  proverit(!s.otpravleno.includes("xhr:https://site.ru/ads/banner.jpg?adunit=1"), "XHR рекламного адреса не отменён");
  const x2 = new s.window.XMLHttpRequest();
  x2.open("GET", "https://site.ru/api/news");
  x2.send();
  proverit(s.otpravleno.includes("xhr:https://site.ru/api/news"), "обычный XHR не ушёл");

  // <img>: рекламный src снимается, обычный остаётся
  const img = s.document.createElement("img");
  img.setAttribute("src", "https://ads.yandex.ru/favicon.ico");
  const nabl = s.observers[s.observers.length - 1];
  nabl.fn([{ addedNodes: [img] }]);
  img.setAttribute("src", "https://site.ru/promo/p.gif?yclid=1");
  for (const o of s.observers) if (o.uzly.some((u) => u.el === img)) o.fn();
  proverit(img.src === "", "рекламный src у img не снят");
  const img2 = s.document.createElement("img");
  nabl.fn([{ addedNodes: [img2] }]);
  img2.setAttribute("src", "https://site.ru/pic/2026.jpg");
  for (const o of s.observers) if (o.uzly.some((u) => u.el === img2)) o.fn();
  proverit(img2.src.includes("/pic/2026.jpg"), "обычная картинка осиротела");

  // ⛔ Сама рекламная страница не должна сама себя резать
  const s2 = sdelatStranicu("https://yandex.ru/ads/help");
  s2.window.Promise = Promise;
  const g2 = vm.createContext(s2.window);
  s2.window.window = g2;
  g2.document = s2.document;
  g2.location = s2.location;
  g2.MutationObserver = s2.window.MutationObserver;
  vm.runInContext(skript, g2);
  let naStranice = true;
  try {
    await s2.window.fetch("https://yandex.ru/ads/help/data.json");
  } catch (e) {
    naStranice = false;
  }
  proverit(naStranice, "своя страница с рекламным путём зарезана — поиск сломан бы был");

  if (oshibki.length) {
    console.error("ПРОВАЛ:\n- " + oshibki.join("\n- "));
    process.exit(1);
  }
  console.log(JSON.stringify({ fetch_reklama_otmenena: true, fetch_obychnyj_ushel: true, xhr: true, img: true, svoya_stranica_ne_rezhetsya: true }));
}

progon().catch((e) => {
  console.error("ПРОВАЛ:", e);
  process.exit(1);
});
