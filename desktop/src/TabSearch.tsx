import {useEffect,useMemo,useRef,useState} from 'react';
import {invoke} from '@tauri-apps/api/core';
import './tabSearch.css';
export type SearchTab={super_id:string;super_name:string;window:string;tab_id:number|null;title:string;address:string;sleeping:boolean;index:number;route:unknown;route_label?:string};
export function matchTabs(rows:SearchTab[],query:string){const parts=query.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);return rows.filter(row=>{const text=`${row.super_name} ${row.title} ${row.address}`.toLocaleLowerCase();return parts.every(part=>text.includes(part));});}
export default function TabSearch({onClose}:{onClose:()=>void}){
 const [rows,setRows]=useState<SearchTab[]>([]),[query,setQuery]=useState(''),[error,setError]=useState(''),[loading,setLoading]=useState(true),[busy,setBusy]=useState(false),[selected,setSelected]=useState(0);
 const input=useRef<HTMLInputElement>(null),alive=useRef(true);
 const matches=useMemo(()=>matchTabs(rows,query),[rows,query]),visible=matches.slice(0,150);
 useEffect(()=>{alive.current=true;void invoke<SearchTab[]>('tab_search_snapshot').then(data=>{if(alive.current)setRows(data);}).catch(e=>{if(alive.current)setError(String(e));}).finally(()=>{if(alive.current)setLoading(false);});void invoke('fokus_obolochke').then(()=>{if(alive.current)input.current?.focus();}).catch(()=>{});return()=>{alive.current=false;};},[]);
 useEffect(()=>{setSelected(0);},[query]);
 useEffect(()=>{document.getElementById(`tab-search-${selected}`)?.scrollIntoView({block:'nearest'});},[selected]);
 async function choose(row:SearchTab){if(busy)return;setBusy(true);setError('');try{await invoke('tab_search_activate',{entry:row});onClose();}catch(e){if(alive.current){setError(String(e));setBusy(false);}}}
 return <div className="zatemnenie" onClick={()=>{if(!busy)onClose();}}><section className="dialog tab-search" role="dialog" aria-modal="true" aria-labelledby="tab-search-title" onClick={e=>e.stopPropagation()} onKeyDown={e=>{if(e.key==='Tab'){const nodes=[...e.currentTarget.querySelectorAll<HTMLElement>('button:not(:disabled),input:not(:disabled)')];const first=nodes[0],last=nodes[nodes.length-1];if(e.shiftKey&&document.activeElement===first){e.preventDefault();last?.focus();}else if(!e.shiftKey&&document.activeElement===last){e.preventDefault();first?.focus();}}if(e.key==='Escape'&&!busy){e.preventDefault();onClose();}if(e.key==='ArrowDown'||e.key==='ArrowUp'){e.preventDefault();setSelected(i=>Math.max(0,Math.min(visible.length-1,i+(e.key==='ArrowDown'?1:-1))));}if(e.key==='Enter'&&visible[selected]){e.preventDefault();void choose(visible[selected]);}}}>
  <header><h2 id="tab-search-title">Поиск по вкладкам</h2><button aria-label="Закрыть поиск по вкладкам" disabled={busy} onClick={onClose}>×</button></header>
  <input ref={input} aria-label="Найти вкладку или супервкладку" placeholder="Название сайта, адрес или супервкладка" value={query} onChange={e=>setQuery(e.target.value)} aria-controls="tab-search-results" aria-activedescendant={visible.length?`tab-search-${selected}`:undefined} autoComplete="off"/>
  {error&&<p role="alert">{error}</p>}
  <small role="status">{loading?'Читаю список вкладок…':busy?'Открываю вкладку…':`${matches.length} найдено${matches.length>150?' · уточните запрос, показаны первые 150':''} · без приватных страниц`}</small>
  <div id="tab-search-results" className="tab-search-results" role="listbox" aria-label="Найденные вкладки">{visible.map((row,i)=><button type="button" role="option" aria-selected={selected===i} id={`tab-search-${i}`} key={`${row.window}/${row.super_id}/${row.tab_id}`} disabled={busy} onMouseEnter={()=>setSelected(i)} onClick={()=>void choose(row)}><strong>{row.title||row.address||row.super_name}</strong><span>{row.super_name}{row.route_label?` · ${row.route_label}`:""}{row.window!=='main'?' · другое окно':''}{row.tab_id===null?' · сохранена':row.sleeping?' · спит':''}</span>{row.address&&<small>{row.address}</small>}</button>)}</div>
  {!loading&&!matches.length&&<p>Ничего не найдено</p>}
 </section></div>;
}
