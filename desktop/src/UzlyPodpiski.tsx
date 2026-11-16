import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
export type Uzel = { imya: string; ssylka: string };
type Proverka = { ms: number | null; status: string };
export default function UzlyPodpiski({ adres }: { adres: string }) {
  const [nodes, setNodes] = useState<Uzel[]>([]), [checks, setChecks] = useState<(Proverka | undefined)[]>([]);
  const [error, setError] = useState(""), [busy, setBusy] = useState(false), [testing, setTesting] = useState(false);
  const generation = useRef(0);
  useEffect(() => () => { generation.current++; }, []);
  if (!/^https?:\/\/[^/]+\/.+/.test(adres)) return null;
  async function measure(list: Uzel[], n: number) {
    setTesting(true);
    for (let i=0;i<list.length && generation.current===n;i++) {
      setChecks((old) => { const next=[...old];next[i]={ms:null,status:"Проверяю…"};return next; });
      let result: Proverka;
      try { result=await invoke<Proverka>("proverit_uzel",{ssylka:list[i].ssylka}); }
      catch { result={ms:null,status:"Не удалось проверить"}; }
      if (generation.current!==n) return;
      setChecks((old) => { const next=[...old];next[i]=result;return next; });
    }
    if (generation.current===n) setTesting(false);
  }
  async function load() {
    const n=++generation.current;setBusy(true);setError("");setChecks([]);setTesting(false);
    try { const list=await invoke<Uzel[]>("uzly_podpiski",{url:adres});if(generation.current===n){setNodes(list);void measure(list,n);} }
    catch(e){if(generation.current===n)setError(String(e));}
    finally{if(generation.current===n)setBusy(false);}
  }
  return <details className="subscription-nodes" onToggle={(e)=>{if(e.currentTarget.open&&!nodes.length&&!busy)void load();}}><summary>Узлы подписки {nodes.length?`(${nodes.length})`:"— показать список"}</summary>
    <p className="muted">Обзор подписки. Задержка измеряется HTTP(S)-запросом через узел, а не до входного сервера. Выбор узла — в настройках конкретной супер-вкладки.</p>
    {busy&&<p role="status">Загружаю список…</p>}{error&&<p role="alert">{error}</p>}
    <div className="node-table">{nodes.map((node,i)=><div className="node-row" key={i}><span>{node.imya}</span><span className={checks[i]?.ms!=null?"node-ok":"muted"}>{checks[i]?.ms!=null?`${checks[i]?.ms} мс`:checks[i]?.status??"Ожидает проверки"}</span></div>)}</div>
    <button type="button" disabled={busy} onClick={()=>void load()}>Обновить список</button>{!!nodes.length&&<button type="button" disabled={busy} onClick={()=>{if(testing){generation.current++;setTesting(false);}else void measure(nodes,++generation.current);}}>{testing?"Остановить проверку":"Проверить задержку"}</button>}
  </details>;
}
