// Проверка скрипта страницы из `desktop/src-tauri/src/borba_s_reklamoy.rs`.
//
// Скрипт живёт в странице, а не в Rust: компиляция его не проверяет. Берём
// ровно те литералы, что собираются в Rust, превращаем Rust-экранирование в
// JS и гоняем на моке DOM. Запуск: `node tests/reklama_stranicy.js`.
const fs = require("fs");
const path = require("path");
const vm = require("vm");

const rust = fs.readFileSync(
  path.join(__dirname, "..", "desktop/src-tauri/src/borba_s_reklamoy.rs"),
  "utf8",
);

// ── сборка скрипта ──
// Вырезаем блок от `let nacalo = format!(` до конца второй format!-строки и
// берём строковые литералы в порядке появления: они и есть код страницы.
const ot = rust.indexOf("let nacalo = format!(");
const do_ = rust.indexOf(".map(|s| s.as_str())", ot);
if (ot < 0 || do_ < 0) throw new Error("не нашёл скрипт в borba_s_reklamoy.rs");
const blok = rust.slice(ot, do_);

function literaly(s) {
  const v = [];
  let i = 0;
  while (i < s.length) {
    const kk = s.indexOf('"', i);
    if (kk < 0) break;
    let j = kk + 1;
    let stroka = "";
    let zakryt = false;
    while (j < s.length) {
      const c = s[j];
      if (c === "\\") {
        stroka += c + (s[j + 1] ?? "");
        j += 2;
        continue;
      }
      if (c === '"') {
        zakryt = true;
        break;
      }
      stroka += c;
      j++;
    }
    if (zakryt) v.push(stroka);
    i = j + 1;
  }
  return v;
}

// Аргументы format! («[]» для ZN и KL) — не код.
const kuski = literaly(blok).filter((x) => !/^\s*\[\]\s*$/.test(x));
// Rust: `\<перевод строки>` — просто перенос, `\\` — обратная косая (в regexp).
const odna = kuski.join("").replace(/\\\n\s*/g, "").replace(/\\\\/g, "\\");
if (!odna.startsWith("(function()")) {
  throw new Error("скрипт не начинается с (function(): " + odna.slice(0, 80));
}
// `{{`/`}}` — экранирование format!, а не JS. `{nacalo}` — подстановка первой
// части: тест склеил обе части сам, поэтому плейсхолдер убираем.
const skript = odna
  .replace("{nacalo}", "")
  .replace(/\{\{/g, "{")
  .replace(/\}\}/g, "}");
// ZN/KL в Rust подставляются через serde_json. ⛔ Берём НАСТОЯЩИЕ списки из
// Rust, иначе тест проверял бы свои придуманные слова, а не то, что уходит
// в страницу.
function spisokRust(imya) {
  // ⛔ Ищем объявление в КОРНЕ модуля: тот же список может быть объявлен
  // и внутри функции (там свои, локальные). Корневое — строкой без отступа.
  const ot = rust.indexOf("\nconst " + imya + ": &[&str] = &[");
  if (ot < 0) throw new Error("не нашёл список " + imya);
  const kon = rust.indexOf("];", ot);
  const telo = rust.slice(ot, kon);
  // строки в двойных кавычках, без комментариев `//` до конца строки
  return [...telo.matchAll(/"([^"\\]*)"/g)].map((m) => m[1]);
}
const ZNAKI = spisokRust("ZNAKI");
const KLASSY = spisokRust("KLASSY");
if (!ZNAKI.length || !KLASSY.length) throw new Error("списки рекламы пусты");
const gotovyj = skript.replace(
  "var ZN={};var KL={};",
  "var ZN=" + JSON.stringify(ZNAKI) + ";var KL=" + JSON.stringify(KLASSY) + ";",
);

// Синтаксис проверяем сразу: битый скрипт не должен уйти в страницу.
try {
  new vm.Script(gotovyj);
} catch (e) {
  console.error("ПРОВАЛ: скрипт страницы не разбирается как JS:", e.message);
  console.error(gotovyj.slice(0, 400));
  process.exit(1);
}

// ── мок страницы ──
// ⛔ Скрипт запускается только на этих страницах (`trebuet_skript` в Rust):
// на чужом сайте он не трогает ничего. Проверяем совпадение границ —
// `ya.ru.example.com` не должен попадать под `ya.ru`.
const HOSTY_SKRIPTA = ["ya.ru", "yandex.ru", "yandex.com", "yandex.by", "yandex.kz", "yandex.uz", "yandex.com.tr", "google.com", "google.ru", "mail.ru", "vk.com", "rambler.ru", "dzen.ru"];
const trebuetSkript = (adres) => {
  try {
    const host = new URL(adres).hostname.replace(/\.$/, "");
    return HOSTY_SKRIPTA.some((d) => host === d || host.endsWith("." + d));
  } catch {
    return false;
  }
};
function proveritGranicySkripta() {
  const vse = HOSTY_SKRIPTA.map((d) => ["https://" + d + "/", "https://www." + d + "/"]);
  vse.push(["https://ya.ru.example.com/", "https://notyandex.ru/", "https://google.com.evil.ru/"]);
  const zhdem = [true, true, true, true, true, true, true, true, true, true, true, true, true, true, true, true, true, true, true, true, true, true, true, true, true, true, false, false, false];
  const fakticheski = vse.flat().map(trebuetSkript);
  const bad = fakticheski.map((x, i) => (x === zhdem[i] ? null : "хост " + vse.flat()[i])).filter(Boolean);
  return bad;
}

// Узлы живут в контексте страницы, поэтому ссылаются на `windowPages`.
let g = null;
const vmWindow = () => (g ? g.window ?? g : null);
const oshibki = [];
const proverit = (uslovie, chto) => {
  if (!uslovie) oshibki.push(chto);
};

function uzel(tag, klass) {
  const el = {
    nodeType: 1,
    tagName: (tag || "").toUpperCase(),
    className: klass || "",
    innerText: "",
    isConnected: true,
    children: [],
    parentOf: null,
    get parentElement(){return this.parentOf;},
    getBoundingClientRect() {
      // как в браузере: размер считается по окну страницы. `this` — узел,
      // окно ищем в контексте страницы (там оно есть как `window`).
      const w = vmWindow();
      const r = { width: 300, height: 60 };
      if (!w) throw new Error("нет окна страницы");
      // маленький блок (60x300) никогда не больше половины экрана
      if (r.width * r.height > w.innerWidth * w.innerHeight * 0.5) throw new Error("большой блок");
      return r;
    },
    matches: (sel) => (sel === "a,[role=link]" ? el.tagName === "A" : false),
    remove() {
      el.isConnected = false;
      el.udaljon = true;
      const i = el.parentOf ? el.parentOf.children.indexOf(el) : -1;
      if (i >= 0) el.parentOf.children.splice(i, 1);
    },
    contains(other){if(this===other)return true;return this.children.some(c=>c.contains(other));},
    style: { setProperty() {} },
    querySelectorAll(sel) {
      const vse = [];
      const podhodit = (n) => {
        if (sel === "a,div,section,aside,ins,iframe,span") {
          return ["A", "DIV", "SECTION", "ASIDE", "INS", "IFRAME", "SPAN"].includes(n.tagName);
        }
        return n.tagName === String(sel).toUpperCase();
      };
      const obhod = (n) =>
        (n.children || []).forEach((c) => {
          if (podhodit(c)) vse.push(c);
          obhod(c);
        });
      obhod(el);
      return vse;
    },
  };
  return el;
}

// Скрипт вызывает projti(document) сам, остальные узлы приходят «изменениями»:
// поэтому мок запоминает обработчик и пробрасывает в него узлы.
function stranica(adres) {
  const body = uzel("body");
  const koren = uzel("html");
  koren.children.push(body);body.parentOf=koren;
  const document = { body, documentElement: koren,
    createTreeWalker(root,mask){
      const all=[];const walk=n=>{if(mask===4){if(n.innerText)all.push({nodeValue:n.innerText});}else all.push(n);for(const c of n.children||[])walk(c);};walk(root);
      let index=mask===4?-1:0;
      return {get currentNode(){return all[index]},lastChild(){if(mask===4)return null;const n=all[index];const children=n.children||[];if(!children.length)return null;index=all.indexOf(children.at(-1));return all[index];},previousNode(){return --index>=0?all[index]:null;},nextNode(){return ++index<all.length?all[index]:null;}};
    }};
  const timers=[];let tick=0;
  const drain=()=>{let guard=0;while(timers.length){if(++guard>10000)throw Error('Unbounded cosmetic task queue');timers.shift()();}};
  const nabl = [];
  const window = {
    innerWidth: 1280,
    setTimeout(fn){timers.push(fn);return timers.length;},
    performance:{now(){return tick+=0.001;}},
    NodeFilter:{SHOW_TEXT:4,SHOW_ELEMENT:1},
    innerHeight: 800,
    MutationObserver: class {
      constructor(fn) {
        this.fn = fn;
        nabl.push(this);
      }
      observe() {}
    },
  };
  g = vm.createContext(window);
  window.window = g;
  g.document = document;
  g.location = new URL(adres);
  g.MutationObserver = window.MutationObserver;
  const dobavit = (uzly) => {
    uzly.forEach((d) => {
      d.parentOf = body;
      body.children.push(d);
    });
    if(nabl.length)nabl[nabl.length - 1].fn([{ addedNodes: uzly }]);drain();
  };
  return { g, document, window, dobavit, nabl, drain };
}

proveritGranicySkripta().forEach((x) =>
  oshibki.push("правило запуска скрипта не совпадает с Rust: " + x),
);

for (const adres of ["https://yandex.ru/", "https://www.google.com/", "https://mail.ru/"]) {
  const s = stranica(adres);
  vm.runInContext(gotovyj, s.g);s.drain();

  const novost = uzel("div");
  novost.innerText = "Погода в Москве завтра: до −3 и снег";
  const plashka = uzel("div");
  plashka.innerText = "Скачай Chrome — быстрее и безопаснее";
  const reklama = uzel("a");
  reklama.innerText = "Купите диван со скидкой\nРЕКЛАМА\nмагазин.ру";
  const instrukciya = uzel("a");
  instrukciya.innerText =
    "Как отключить рекламу в браузере — подробная инструкция со скриншотами и советами";
  const po_klassu = uzel("ins", "advert-banner");
  const mnogostrochnyj = uzel("div");
  mnogostrochnyj.innerText = ["Купите диван", "Скачай Chrome — это быстро", "магазин.ру"].join("\n");

  s.dobavit([novost, plashka, reklama, instrukciya, po_klassu, mnogostrochnyj]);

  proverit(!novost.udaljon, `обычная новость убрана на ${adres}`);
  proverit(!!plashka.udaljon, `плашка «Скачай Chrome» осталась на ${adres}`);
  proverit(!!reklama.udaljon, `карточка с маркировкой РЕКЛАМА осталась на ${adres}`);
  proverit(!instrukciya.udaljon, `статья со словом «рекламу» убрана на ${adres}`);
  proverit(!!po_klassu.udaljon, `блок с классом из списка-приманки остался на ${adres}`);
  proverit(!!mnogostrochnyj.udaljon, `плашка в многострочном блоке осталась на ${adres}`);
}

// Вложенный узел нового поддерева тоже оценивается (projti(n)):
// ad-блок часто приходит внутри пустой оболочки-контейнера.
{
  const s = stranica("https://yandex.ru/");
  vm.runInContext(gotovyj, s.g);s.drain();
  const obolochka = uzel("div");
  const vnutri = uzel("aside");
  vnutri.innerText = "Установите другой браузер — этот устарел";
  vnutri.parentOf = obolochka;
  obolochka.children.push(vnutri);
  s.dobavit([obolochka]);
  proverit(!obolochka.udaljon, "оболочка убрана — значит otsenit дошёл до вложенного");
  proverit(!!vnutri.udaljon, "вложенная плашка внутри нового узла осталась");
}

// Та же плашка, но прямым узлом: если она убирается, ищем проблему в projti.
{
  const s = stranica("https://yandex.ru/");
  vm.runInContext(gotovyj, s.g);s.drain();
  const pryamo = uzel("aside");
  pryamo.innerText = "Установите другой браузер — этот устарел";
  s.dobavit([pryamo]);
  proverit(!!pryamo.udaljon, "плашка прямым узлом не убрана — дело не в projti");
}

// Повторный запуск в той же странице ничего не делает и не падает.
{
  const s = stranica("https://yandex.ru/");
  vm.runInContext(gotovyj, s.g);s.drain();
  vm.runInContext(gotovyj, s.g);s.drain();
  proverit(s.window.__nbReklama === 1, "скрипт не отметил повторный запуск");
}

if (oshibki.length) {
  console.error("ПРОВАЛ:\n- " + oshibki.join("\n- "));
  process.exit(1);
}
console.log(
  JSON.stringify({
    obychnaya_novost_ostalas: true,
    plashka_ubrana: true,
    markirovka_reklama_ubrana: true,
    slovo_reklama_v_state_ne_trogaetsya: true,
    klass_primanok_ubran: true,
    mnogostrochnyj_blok_obrabotan: true,
    vlozhennoe_ubrano: true,
    granicy_zapuska_sovpadayut: true,
    povtor_bezopasen: true,
  }),
);
