import { useMemo, useState } from "react";
import Icon from "./Icons";
export type Zhurnal = { vklyuchena: boolean; zarezano: number; propushcheno: number; pravila_vsego: number; summy: [string, number][]; zapisi: { domen: string; pravilo: string; metod: string; vremya_ms: number }[] };
type Opisanie = { servis: string; kategoriya: string; zachem: string };
const ads = "Реклама", stats = "Аналитика", social = "Социальные сети", telemetry = "Диагностика", tags = "Управление тегами";
const catalog: [string[], string, string, string][] = [
  [["doubleclick.net","googlesyndication.com","googleadservices.com","adservice.google.com","pagead2.googlesyndication.com","googletagservices.com"],"Google Ads / DoubleClick",ads,"Показ рекламы, рекламные аукционы и измерение эффективности объявлений."],
  [["google-analytics.com","analytics.google.com"],"Google Analytics",stats,"Сбор статистики посещений и действий на сайтах."],
  [["googletagmanager.com"],"Google Tag Manager",tags,"Загрузка тегов аналитики и рекламы. Не каждый тег является рекламой."],
  [["mc.yandex.ru"],"Яндекс Метрика",stats,"Счётчик посещений и событий, аналитика поведения на сайте."],
  [["an.yandex.ru"],"Рекламная сеть Яндекса",ads,"Запросы к рекламной сети и её рекламным материалам."],
  [["adfox.ru","ads.adfox.ru"],"Adfox",ads,"Управление показами рекламы и рекламными кампаниями."],
  [["top-fwz1.mail.ru"],"Счётчик Mail.ru",stats,"Статистика посещаемости и аудитории сайтов."],
  [["ad.mail.ru","rb.mail.ru"],"Реклама Mail.ru",ads,"Показ и измерение рекламы рекламной сети."],
  [["counter.rambler.ru","top100.rambler.ru"],"Рамблер / Топ-100",stats,"Счётчик посещаемости и статистика аудитории."],
  [["smi2.ru"],"СМИ2",ads,"Сеть новостных рекомендаций и рекламных переходов."],
  [["luckyads.pro"],"LuckyAds",ads,"Рекламные рекомендации и продвижение материалов."],
  [["relap.io"],"Relap",ads,"Рекомендации материалов и рекламные блоки."],
  [["adnxs.com"],"AppNexus / Xandr",ads,"Рекламные аукционы, подбор и показ объявлений."],
  [["adsrvr.org"],"The Trade Desk",ads,"Рекламная платформа: подбор аудитории и измерение рекламы."],
  [["criteo.com","criteo.net"],"Criteo",ads,"Персонализированная реклама и повторный показ объявлений."],
  [["taboola.com"],"Taboola",ads,"Рекламные рекомендации материалов и объявлений."],
  [["outbrain.com"],"Outbrain",ads,"Рекомендации и продвижение рекламных материалов."],
  [["pubmatic.com"],"PubMatic",ads,"Автоматизированная продажа и показ рекламы."],
  [["rubiconproject.com"],"Rubicon Project",ads,"Рекламная биржа и аукционы рекламных мест."],
  [["openx.net"],"OpenX",ads,"Рекламная биржа и подбор объявлений."],
  [["casalemedia.com"],"Casale Media",ads,"Рекламная биржа и доставка объявлений."],
  [["sharethrough.com"],"Sharethrough",ads,"Рекламная платформа для размещения объявлений."],
  [["smartadserver.com"],"Smart AdServer",ads,"Сервер показа и измерения рекламы."],
  [["teads.tv"],"Teads",ads,"Рекламная платформа, в том числе видеореклама."],
  [["33across.com"],"33Across",ads,"Рекламная платформа и работа с рекламной аудиторией."],
  [["bidswitch.net"],"BidSwitch",ads,"Обмен запросами между участниками рекламных аукционов."],
  [["connect.facebook.net"],"Meta / Facebook",social,"Загрузка социальных и измерительных скриптов. Блокировка может затронуть виджеты."],
  [["ads.twitter.com"],"Реклама Twitter / X",ads,"Рекламные запросы и измерение кампаний."],
  [["analytics.tiktok.com"],"Аналитика TikTok",stats,"Измерение событий и эффективности рекламных переходов."],
  [["ads.tiktok.com"],"Реклама TikTok",ads,"Запросы к рекламной платформе TikTok."],
  [["scorecardresearch.com"],"ScorecardResearch",stats,"Измерение аудитории и посещаемости."],
  [["quantserve.com"],"Quantcast",stats,"Измерение аудитории и рекламная аналитика."],
  [["hotjar.com"],"Hotjar",stats,"Аналитика поведения пользователей и взаимодействия со страницей."],
  [["mixpanel.com"],"Mixpanel",stats,"Продуктовая аналитика: события и действия пользователей."],
  [["segment.io"],"Segment",stats,"Сбор и передача событий в системы аналитики."],
  [["amplitude.com"],"Amplitude",stats,"Продуктовая аналитика и измерение пользовательских событий."],
  [["branch.io"],"Branch",stats,"Измерение переходов и источников привлечения пользователей."],
  [["appsflyer.com"],"AppsFlyer",stats,"Атрибуция установок, переходов и рекламных кампаний."],
  [["adjust.com"],"Adjust",stats,"Атрибуция и измерение рекламных кампаний."],
  [["sentry.io"],"Sentry",telemetry,"Отчёты об ошибках приложения. Это техническая телеметрия, а не рекламный баннер."],
  [["bugsnag.com"],"Bugsnag",telemetry,"Отправка отчётов об ошибках и сбоях сайта."],
  [["newrelic.com","nr-data.net"],"New Relic",telemetry,"Мониторинг производительности и техническая телеметрия."],
];
export function opisanie(rule: string): Opisanie {
  const row = catalog.find(([rules]) => rules.includes(rule));
  return row ? { servis: row[1], kategoriya: row[2], zachem: row[3] } : { servis: rule, kategoriya: "Другие сервисы", zachem: "Соединение совпало со встроенным списком нежелательных доменов." };
}
export default function Blokirovki({ journal, name, onClose }: { journal: Zhurnal; name: string; onClose: () => void }) {
  const [search, setSearch] = useState(""); const [category, setCategory] = useState(""); const [view, setView] = useState("summary");
  const services = useMemo(() => {
    const grouped = new Map<string, Opisanie & { count: number; rules: string[] }>();
    for (const [rule, count] of journal.summy ?? []) {
      const info = opisanie(rule); const prior = grouped.get(info.servis);
      grouped.set(info.servis, { ...info, count: count + (prior?.count ?? 0), rules: [...prior?.rules ?? [], rule] });
    }
    return [...grouped.values()].sort((a,b) => b.count-a.count);
  }, [journal.summy]);
  const fits = (info: Opisanie, text: string) => (!category || category === info.kategoriya) && (info.servis + " " + text).toLowerCase().includes(search.toLowerCase());
  return <div className="zatemnenie" onClick={onClose} onKeyDown={(e) => { if (e.key === "Escape") onClose(); }}><section className="dialog protection-dialog" role="dialog" aria-label="Защита супер-вкладки" onClick={(e) => e.stopPropagation()}>
    <header className="settings-header"><div><span className="eyebrow">ЗАЩИТА · {name}</span><h2>Что заблокировано</h2><p className="muted">{journal.vklyuchena ? "Блокировка включена" : "Блокировка выключена"} · Все обычные вкладки этого профиля</p></div><button className="icon-button" aria-label="Закрыть журнал защиты" onClick={onClose}><Icon name="close" /></button></header>
    <div className="protection-stats"><div><strong>{journal.zarezano}</strong><small>отклонено соединений</small></div><div><strong>{services.length}</strong><small>нежелательных сервисов</small></div><div><strong>{journal.propushcheno}</strong><small>пропущено соединений</small></div></div>
    <p className="settings-tip">Это число соединений, а не баннеров: повторные попытки считаются отдельно. Встроенный список NexusBro: {journal.pravila_vsego} доменных правил.</p>
    <div className="protection-controls"><div className="protection-views"><button className={view === "summary" ? "selected" : ""} onClick={() => setView("summary")}>По сервисам</button><button className={view === "events" ? "selected" : ""} onClick={() => setView("events")}>События</button></div><input aria-label="Поиск в журнале защиты" placeholder="Домен или сервис…" value={search} onChange={(e) => setSearch(e.target.value)} /><select aria-label="Категория блокировки" value={category} onChange={(e) => setCategory(e.target.value)}><option value="">Все категории</option>{[ads,stats,social,tags,telemetry,"Другие сервисы"].map((c) => <option key={c}>{c}</option>)}</select></div>
    <div className="block-list">{view === "summary" ? services.filter((s) => fits(s,s.rules.join(" "))).map((s) => <details className="service-entry" key={s.servis}><summary><Icon name="shield" size={18} /><div><strong>{s.servis}</strong><small>{s.kategoriya}</small></div><b>{s.count}</b></summary><p>{s.zachem}</p><small>Причина: домен соединения совпал с правилом встроенного списка.</small><div className="rule-tags">{s.rules.map((r) => <code key={r}>{r}</code>)}</div></details>) : journal.zapisi.filter((r) => fits(opisanie(r.pravilo),r.domen)).map((r,i) => { const info = opisanie(r.pravilo); return <details className="block-entry" key={r.vremya_ms+":"+i}><summary><Icon name="shield" size={16} /><div><strong>{info.servis}</strong><small>{r.domen}</small></div><time>{new Date(r.vremya_ms).toLocaleTimeString()}</time></summary><p>{info.zachem}</p><dl><dt>Категория</dt><dd>{info.kategoriya}</dd><dt>Действие</dt><dd>Соединение отклонено до выхода в сеть</dd><dt>Протокол</dt><dd>{r.metod === "CONNECT" ? "HTTPS (CONNECT)" : "HTTP · " + r.metod}</dd><dt>Правило</dt><dd><code>{r.pravilo}</code> и поддомены</dd><dt>Источник</dt><dd>Встроенный список NexusBro</dd></dl></details>; })}</div>
    {!journal.zarezano && <p className="muted">За этот запуск заблокированных соединений ещё нет.</p>}
    <small className="muted">Итоги — за текущий запуск профиля; события — последние 200. Обновляется автоматически. Журнал хранится в памяти. Содержимое HTTPS не читается: конкретный баннер и сайт-инициатор по соединению не определяются.</small>
    <div className="knopki"><button onClick={onClose}>Закрыть</button></div>
  </section></div>;
}
