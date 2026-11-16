import { Children, isValidElement, CSSProperties, ReactNode, useEffect, useId, useRef, useState } from "react";

import { createPortal } from "react-dom";

type Props = { value: string | number; onChange: (e: { target: { value: string } }) => void; children: ReactNode; disabled?: boolean; className?: string; "aria-label"?: string };
// DOM-список вместо native select: popup WebView2 терял фокус над встроенным CEF.
export default function Vybor(p: Props) {
  const options = Children.toArray(p.children).filter(isValidElement).map((c) => {
    const v = c.props as { value?: string | number; children?: ReactNode; disabled?: boolean };
    return { value: String(v.value ?? v.children ?? ""), label: v.children, disabled: v.disabled };
  });
  const [open, setOpen] = useState(false), [index, setIndex] = useState(0), [search, setSearch] = useState("");
  const popup = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState<(CSSProperties & { "--options-height": string }) | null>(null);
  const root = useRef<HTMLDivElement>(null), button = useRef<HTMLButtonElement>(null), id = useId();
  const selected = options.find((o) => o.value === String(p.value));
  const visible = options.filter((o) => String(o.label).toLocaleLowerCase().includes(search.toLocaleLowerCase()));
  function close() { setOpen(false); setSearch(""); setPosition(null); }
  function choose(i: number) { const o = visible[i]; if (o && !o.disabled) { p.onChange({ target: { value: o.value } }); close(); button.current?.focus(); } }
  useEffect(() => {
    if (!open) return;
    const r = button.current?.getBoundingClientRect();
    if (r) {
      const below = window.innerHeight-r.bottom-12, above = r.top-12;
      const up = below < Math.min(300,above);
      const space = Math.max(100,up ? above : below), width = Math.min(window.innerWidth-24,Math.max(180,r.width));
      setPosition({ position:"fixed", left:Math.max(12,Math.min(r.left,window.innerWidth-width-12)), width,
        ...(up ? { bottom:window.innerHeight-r.top+4 } : { top:r.bottom+4 }),
        "--options-height":Math.max(40,Math.min(220,space-64))+"px" });
    }
    const outside = (e: PointerEvent) => { if (!root.current?.contains(e.target as Node) && !popup.current?.contains(e.target as Node)) close(); };
    document.addEventListener("pointerdown", outside);
    return () => document.removeEventListener("pointerdown", outside);
  }, [open]);
  useEffect(() => { popup.current?.querySelector('[data-highlight="true"]')?.scrollIntoView({ block: "nearest" }); }, [index, open, position]);
  return <div className={"vybor " + (p.className ?? "")} ref={root} onClick={(e) => e.stopPropagation()} onKeyDown={(e) => {
    if (e.key === "Escape" && open) { e.preventDefault(); e.stopPropagation(); close(); button.current?.focus(); }
    else if (["ArrowDown","ArrowUp","Home","End"].includes(e.key)) {
      e.preventDefault(); setOpen(true); setIndex(e.key === "Home" ? 0 : e.key === "End" ? visible.length - 1 : Math.max(0,Math.min(visible.length - 1,index + (e.key === "ArrowDown" ? 1 : -1))));
    } else if ((e.key === "Enter" || e.key === " ") && e.target === button.current) { e.preventDefault(); if (open) choose(index); else { setIndex(Math.max(0,options.findIndex((o) => o.value === String(p.value)))); setOpen(true); } }
    else if (e.key === "Enter" && open) { e.preventDefault(); choose(index); }
    else if (e.key === "Tab") close();
  }}>
    <button type="button" ref={button} role="combobox" aria-label={p["aria-label"]} aria-expanded={open} aria-controls={id} aria-haspopup="listbox" disabled={p.disabled} onClick={() => { if (open) close(); else { setIndex(Math.max(0,options.findIndex((o) => o.value === String(p.value)))); setOpen(true); } }}>{selected?.label ?? "Выберите…"}<span aria-hidden="true">⌄</span></button>
    {open && position && createPortal(<div className="vybor-popup" ref={popup} style={position}><input aria-label="Поиск в списке" placeholder="Найти…" value={search} onChange={(e) => { setSearch(e.target.value); setIndex(0); }} /><div role="listbox" id={id} aria-label={p["aria-label"]}>{visible.map((o,i) => <button type="button" role="option" aria-selected={o.value === String(p.value)} disabled={o.disabled} data-highlight={i === index} key={o.value + i} onPointerMove={() => setIndex(i)} onClick={() => choose(i)}>{o.label}</button>)}{!visible.length && <small>Ничего не найдено</small>}</div></div>,document.body)}
  </div>;
}
