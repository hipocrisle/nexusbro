import ImportWizard from "./ImportWizard";
import {shortcutKey} from "./shortcuts";
import {useEffect,useRef,useState} from 'react';
import {invoke} from '@tauri-apps/api/core';
import {listenWindow as listen} from './windowEvents';
type Zapis={id:string;adres:string;nazvanie:string;vremya:number;papka:string};
type Download={id:string;super_id:string;name:string;url:string;path:string;percent:number;received:number;total:number;speed:number;status:string;privatnaya:boolean};
type Razdel='istoriya'|'zakladki'|'zagruzki';
function sourceHost(url:string){try{return new URL(url).hostname;}catch{return 'Источник неизвестен';}}
const titles={istoriya:'История',zakladki:'Закладки',zagruzki:'Загрузки'};
export default function Biblioteka({superId}:{superId:string|null}){
 const [open,setOpen]=useState<Razdel|null>(null),[items,setItems]=useState<Zapis[]>([]),[downloads,setDownloads]=useState<Download[]>([]);
 const [importOpen,setImportOpen]=useState(false);
 const [query,setQuery]=useState(''),[error,setError]=useState(''),[info,setInfo]=useState('');
 const input=useRef<HTMLInputElement>(null);
 async function show(section:Razdel){await invoke('pokazat_dvizhki',{vidno:false});await invoke('fokus_obolochke');setQuery('');setError('');setInfo('');setOpen(section);}
 async function close(){setOpen(null);await invoke('pokazat_dvizhki',{vidno:true});}
 async function refresh(){if(!open)return;try{if(open==='zagruzki')setDownloads(await invoke<Download[]>('zagruzki_spisok'));else setItems(await invoke<Zapis[]>('biblioteka_spisok',{razdel:open}));}catch(e){setError(String(e));}}
 useEffect(()=>{const open=(e:Event)=>void show((e as CustomEvent<Razdel>).detail);window.addEventListener('nexusbro-library',open);return()=>window.removeEventListener('nexusbro-library',open);},[]);
 useEffect(()=>{const key=(e:KeyboardEvent)=>{if(e.shiftKey||e.altKey||!(e.ctrlKey||e.metaKey))return;const s=({h:'istoriya',j:'zagruzki',b:'zakladki'} as Record<string,Razdel>)[shortcutKey(e)];if(s&&!document.querySelector('.zatemnenie')){e.preventDefault();void show(s);}};document.addEventListener('keydown',key,true);return()=>document.removeEventListener('keydown',key,true);},[]);
 useEffect(()=>{const off=listen<{super_id:string;data:{tip:string;razdel?:Razdel}}>('stranica-ui',e=>{if(e.payload.super_id===superId&&e.payload.data.tip==='panel'&&e.payload.data.razdel)void show(e.payload.data.razdel);});return()=>{void off.then(f=>f());};},[superId]);
 useEffect(()=>{if(!open)return;void refresh();input.current?.focus();const timer=open==='zagruzki'?setInterval(()=>void refresh(),750):undefined;return()=>{if(timer)clearInterval(timer);};},[open]);
 async function remove(id:string|null){if(!open)return;try{await invoke('biblioteka_udalit',{razdel:open,id});await refresh();}catch(e){setError(String(e));}}
 async function navigate(url:string){if(!superId)return;try{await invoke('obychnaya_komanda',{id:superId,komanda:{deystvie:'novaya',adres:url}});await close();}catch(e){setError(String(e));}}
 async function importFile(file:File|undefined){if(!file)return;try{
  if(file.size>8*1024*1024)throw new Error('Файл слишком большой');
  const doc=new DOMParser().parseFromString(await file.text(),'text/html');
  const zapisi=[...doc.querySelectorAll('a[href]')].filter(a=>/^https?:\/\//i.test(a.getAttribute('href')??'')).map((a,i)=>({id:'import-'+i,adres:a.getAttribute('href')!,nazvanie:a.textContent?.trim()||a.getAttribute('href')!,vremya:Number(a.getAttribute('add_date'))||Math.floor(Date.now()/1000),papka:'Импортированные'}));
  const n=await invoke<number>('zakladki_import',{zapisi});setInfo(`Импортировано: ${n}`);await refresh();
 }catch(e){setError(String(e));}}
 async function clearDownloads(id:string|null){try{await invoke('zagruzki_ochistit',{id});await refresh();}catch(e){setError(String(e));}}
 async function downloadAction(d:Download,action:string){try{await invoke('zagruzka_deystvie',{superId:d.super_id,id:d.id,action});await refresh();}catch(e){setError(String(e));}}
 return <>

  {open&&<div className="zatemnenie" onClick={()=>void close()}><section className="dialog library-panel" role="dialog" aria-label={titles[open]} onClick={e=>e.stopPropagation()} onKeyDown={e=>{if(e.key==='Escape')void close();}}>
   <header><h2>{titles[open]}</h2><button aria-label="Закрыть библиотеку" onClick={()=>void close()}>×</button></header>
   <nav>{(Object.keys(titles) as Razdel[]).map(s=><button className={s===open?'selected':''} key={s} onClick={()=>setOpen(s)}>{titles[s]}</button>)}</nav>
   <input ref={input} aria-label="Поиск в библиотеке" value={query} onChange={e=>setQuery(e.target.value)} placeholder="Название или адрес…"/>
   {error&&<p role="alert">{error}</p>}{info&&<p role="status">{info}</p>}
   {open==='zakladki'&&<><button onClick={()=>setImportOpen(v=>!v)}>Перенести закладки из другого браузера…</button>{importOpen&&<><ImportWizard onDone={()=>void refresh()}/><details><summary>Ручной импорт из файла HTML</summary><label className="import-bookmarks">Выбрать файл закладок<input type="file" accept=".html,.htm,text/html" onChange={e=>void importFile(e.target.files?.[0])}/></label></details></>}</>}
   {open!=='zagruzki'?<div className="library-list">{items.filter(i=>(i.nazvanie+' '+i.adres).toLocaleLowerCase().includes(query.toLocaleLowerCase())).slice(0,200).map(i=><article key={i.id}><button className="library-open" disabled={!superId} onClick={()=>void navigate(i.adres)}><strong>{i.nazvanie||i.adres}</strong><small>{i.adres}</small><small>{open==='istoriya'?new Date(i.vremya*1000).toLocaleString():i.papka}</small></button><button title="Удалить запись" onClick={()=>void remove(i.id)}>×</button></article>)}{items.length===0&&<p>Пока пусто</p>}</div>:<div className="library-list">{downloads.filter(d=>(d.name+' '+sourceHost(d.url)).toLowerCase().includes(query.toLowerCase())).map(d=><article key={d.super_id+':'+d.id}><div className="download-row"><strong>{d.name||'Загрузка'}{d.privatnaya?' · приватная':''}</strong><small>{d.status} · {Math.round(d.received/1024)} КБ{d.total>0?` из ${Math.round(d.total/1024)} КБ`:''}</small>{d.status==='Загружается'&&<progress max="100" value={Math.max(0,d.percent)}/>}<small className="download-source">{sourceHost(d.url)}</small><div>{['Загружается','Приостановлена'].includes(d.status)?<><button onClick={()=>void downloadAction(d,d.status==='Приостановлена'?'resume':'pause')}>{d.status==='Приостановлена'?'Продолжить':'Пауза'}</button><button onClick={()=>void downloadAction(d,'cancel')}>Отменить</button></>:<button onClick={()=>void downloadAction(d,'retry')}>Повторить</button>}{!['Загружается','Приостановлена'].includes(d.status)&&<button title="Удалить из списка, оставить файл" onClick={()=>void clearDownloads(d.id)}>Убрать из списка</button>}{d.path&&<button onClick={()=>void downloadAction(d,'folder')}>Показать в папке</button>}</div></div></article>)}</div>}
   {open==='zagruzki'&&<footer><span>Удаление из списка не удаляет файлы. Активные загрузки остаются.</span><button onClick={()=>void clearDownloads(null)}>Очистить список</button></footer>}
   {open==='istoriya'&&<footer><span>Приватные страницы здесь не сохраняются. Показано до 200 записей; используйте поиск.</span><button onClick={()=>void remove(null)}>Очистить историю</button></footer>}
  </section></div>}
 </>;
}
