import { Children, isValidElement, ReactNode, useEffect, useId, useRef, useState } from "react";

type Props = { value: string | number; onChange: (e: { target: { value: string } }) => void; children: ReactNode; disabled?: boolean; className?: string; "aria-label"?: string };
// DOM-список вместо native select: popup WebView2 терял фокус над встроенным CEF.
export default function Vybor(p: Props) {
  const options = Children.toArray(p.children).filter(isValidElement).map((c) => {
    const v = c.props as { value?: string | number; children?: ReactNode; disabled?: boolean };
    return { value: String(v.value ?? v.children ?? ""), label: v.children, disabled: v.disabled };
  });
  const [open, setOpen] = useState(false), [index, setIndex] = useState(0), [search, setSearch] = useState("");
  const root = useRef<HTMLDivElement>(null), button = useRef<HTMLButtonElement>(null), id = useId();
  const selected = options.find((o) => o.value === String(p.value));
  const visible = options.filter((o) => String(o.label).toLocaleLowerCase().includes(search.toLocaleLowerCase()));
  function close() { setOpen(false); setSearch(""); }
  function choose(i: number) { const o = visible[i]; if (o && !o.disabled) { p.onChange({ target: { value: o.value } }); close(); button.current?.focus(); } }
  useEffect(() => {
    if (!open) return;
    const outside = (e: PointerEvent) => { if (!root.current?.contains(e.target as Node)) close(); };
    document.addEventListener("pointerdown", outside);
    return () => document.removeEventListener("pointerdown", outside);
  }, [open]);
  useEffect(() => { root.current?.querySelector('[data-highlight="true"]')?.scrollIntoView({ block: "nearest" }); }, [index, open]);
  return <div className={"vybor " + (p.className ?? "")} ref={root} onClick={(e) => e.stopPropagation()} onKeyDown={(e) => {
    if (e.key === "Escape" && open) { e.preventDefault(); e.stopPropagation(); close(); button.current?.focus(); }
    else if (["ArrowDown","ArrowUp","Home","End"].includes(e.key)) {
      e.preventDefault(); setOpen(true); setIndex(e.key === "Home" ? 0 : e.key === "End" ? visible.length - 1 : Math.max(0,Math.min(visible.length - 1,index + (e.key === "ArrowDown" ? 1 : -1))));
    } else if ((e.key === "Enter" || e.key === " ") && e.target === button.current) { e.preventDefault(); if (open) choose(index); else { setIndex(Math.max(0,options.findIndex((o) => o.value === String(p.value)))); setOpen(true); } }
    else if (e.key === "Enter" && open) { e.preventDefault(); choose(index); }
    else if (e.key === "Tab") close();
  }}>
    <button type="button" ref={button} role="combobox" aria-label={p["aria-label"]} aria-expanded={open} aria-controls={id} aria-haspopup="listbox" disabled={p.disabled} onClick={() => { if (open) close(); else { setIndex(Math.max(0,options.findIndex((o) => o.value === String(p.value)))); setOpen(true); } }}>{selected?.label ?? "Выберите…"}<span aria-hidden="true">⌄</span></button>
    {open && <div className="vybor-popup"><input aria-label="Поиск в списке" placeholder="Найти…" value={search} onChange={(e) => { setSearch(e.target.value); setIndex(0); }} /><div role="listbox" id={id} aria-label={p["aria-label"]}>{visible.map((o,i) => <button type="button" role="option" aria-selected={o.value === String(p.value)} disabled={o.disabled} data-highlight={i === index} key={o.value + i} onPointerMove={() => setIndex(i)} onClick={() => choose(i)}>{o.label}</button>)}{!visible.length && <small>Ничего не найдено</small>}</div></div>}
  </div>;
}
