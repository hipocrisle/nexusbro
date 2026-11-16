import { useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import Vybor from "./Vybor";
export type Uzel = { imya: string; ssylka: string };
export default function UzlyPodpiski({ adres, value, onChange }: { adres: string; value: string; onChange: (v: string) => void }) {
  const [nodes, setNodes] = useState<Uzel[]>([]), [error, setError] = useState(""), [busy, setBusy] = useState(false);
  const generation = useRef(0);
  if (!/^https?:\/\/[^/]+\/.+/.test(adres)) return null;
  async function load() {
    const n = ++generation.current; setBusy(true); setError("");
    try { const list = await invoke<Uzel[]>("uzly_podpiski", { url: adres }); if (generation.current === n) setNodes(list); }
    catch (e) { if (generation.current === n) setError(String(e)); }
    finally { if (generation.current === n) setBusy(false); }
  }
  return <details className="subscription-nodes" onToggle={(e) => { if (e.currentTarget.open && !nodes.length && !busy) void load(); }}><summary>Узлы подписки {nodes.length ? `(${nodes.length})` : "— загрузить и выбрать"}</summary>
    <p className="muted">Узел по умолчанию для новых супер-вкладок с этим подключением.</p>
    {busy && <p role="status">Загружаю список…</p>}{error && <p role="alert">{error}</p>}
    <Vybor aria-label="Узел сохранённой подписки" value={value} onChange={(e) => onChange(e.target.value)} disabled={busy}><option value="">Первый узел подписки</option>{value && !nodes.some((n) => n.ssylka === value) && <option value={value}>Сохранённый узел</option>}{nodes.map((n,i) => <option value={n.ssylka} key={i}>{n.imya}</option>)}</Vybor>
    <button type="button" disabled={busy} onClick={() => void load()}>Обновить список узлов</button>
  </details>;
}
