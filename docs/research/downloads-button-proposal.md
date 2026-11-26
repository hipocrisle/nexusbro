# Downloads button у адресной строки — предложение (подготовка, без интеграции)

Ветка test/downloads-shell-window-proof (worktree /matrix/nexusbro-kimi-downloads-runtime).
Файлы: НОВЫЕ `desktop/src/DownloadsButton.tsx`, `desktop/src/downloadsButton.css`,
`tests/downloads_button_contract_test.py`, эта документация. Существующие файлы
НЕ менялись; интеграция — только ROOT (production freeze, producer 45780933571
собирает неизменяемый source 7b8).

## Компонент

Компактная стандартная кнопка загрузок для вставки рядом с `TabRouteButton` в
`ObychnyeVkladki.tsx`. Props: `superId`, `privatnaya`, `onOpen`.

- Обновление: mount + событие-инвалидация `downloads-changed` (без содержимого)
  → `invoke zagruzki_spisok` с точным фильтром `super_id===superId &&
  privatnaya===privatnaya`. Никакого idle poll-таймера; throttle ≤4 Гц
  (250 мс); очистка listener/timeout при unmount; устаревшие ответы отсекаются
  счётчиком поколения.
- Индикация: активных N + взвешенный реальный процент ТОЛЬКО если у ВСЕХ
  активных известен total; иначе indeterminate-полоска (без процента).
  Пауза — отдельный приглушённый стиль, статусы не искажаются. Клик — только
  `onOpen` (открывает панель библиотеки), открытую страницу не заменяет.
- Приватность: в badge/tooltip только количества и процент — никаких имён,
  URL, путей. Приватные загрузки не видны обычным вкладкам (точное совпадение
  флага privatnaya).
- Стиль: тёмный, в духе проектных Icon-SVG (inline svg стрелка-в-лоток),
  без глобального busy-индикатора и без насыщенных анимаций.

## DEPENDENCY (за ROOT, не обещается «живым» до события)

Backend generic-инвалидация `downloads-changed` после обновления хранилища
(`obnovit` в zagruzki.rs → событие окну) — файл ROOT'а (lib.rs), я его не
трогаю. До появления события кнопка честно обновляется на mount и по
событиям; live-статус не заявляется. Предлагаемая точка эмиссии: после
`zapisat(&m)` в `obnovit` — `app.emit_to(..., "downloads-changed", ())` без
полезной нагрузки.

## Проверки (честные границы)

- `tests/downloads_button_contract_test.py`: 17 статических проверок PASS
  (нет idle-poll, throttle, generation-guard, cleanup, точный фильтр,
  badge только counts, отсутствие url/path/name, indeterminate при
  неизвестных total, paused-стиль, dark-only, без глобального busy).
- esbuild parse TSX: OK. tsc/нативный рендер не выполнялись (нет
  node_modules/запрет GUI) — static не выдаётся за native.

## Интеграция (за ROOT)

```tsx
// ObychnyeVkladki.tsx, рядом с <TabRouteButton .../>
<DownloadsButton superId={id} privatnaya={!!aktivnaya?.privatnaya}
  onOpen={()=>window.dispatchEvent(new CustomEvent('nexusbro-library',{detail:'zagruzki'}))}/>
```
