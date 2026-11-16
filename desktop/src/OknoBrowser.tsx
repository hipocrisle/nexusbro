import {useEffect,useRef,useState} from 'react';
import {invoke} from '@tauri-apps/api/core';
import {listenWindow as listen} from './windowEvents';
import ObychnyeVkladki from './ObychnyeVkladki';
import Biblioteka from './Biblioteka';
import Blokirovki,{type Zhurnal} from './Blokirovki';
import {poUmolchaniyu,shrifty,type NastrojkiApp} from './Nastrojki';
import {editableTarget} from './addressSelection';

export default function OknoBrowser(){
 const id=new URLSearchParams(location.search).get('super')!;
 const [settings,setSettings]=useState<NastrojkiApp>(poUmolchaniyu),[error,setError]=useState('');
 const [name,setName]=useState('Супер-вкладка'),[journal,setJournal]=useState<Zhurnal|null>(null);
 const content=useRef<HTMLDivElement>(null),started=useRef(false);
 useEffect(()=>{
  void invoke<{id:string;nazvanie:string}[]>('spisok').then(rows=>setName(rows.find(w=>w.id===id)?.nazvanie||'Супер-вкладка')).catch(()=>{});
  void invoke<NastrojkiApp>('nastrojki_poluchit').then(setSettings).catch(e=>setError(String(e)));
  if(!started.current){started.current=true;void invoke('okno_gotovo').catch(e=>setError(String(e)));}
  const measure=()=>{const r=content.current?.getBoundingClientRect();if(r){const k=devicePixelRatio||1;void invoke('zadat_mesto',{x:Math.round(r.x*k),y:Math.round(r.y*k),w:Math.round(r.width*k),h:Math.round(r.height*k)}).catch(e=>setError(String(e)));}};
  const observer=new ResizeObserver(measure);if(content.current)observer.observe(content.current);window.addEventListener('resize',measure);measure();
  const off=listen<string>('oshibka-dvizhka',e=>setError(e.payload));
  return()=>{observer.disconnect();window.removeEventListener('resize',measure);void off.then(f=>f());};
 },[]);
 useEffect(()=>{if(!journal)return;const timer=setInterval(()=>void invoke<Zhurnal>('zhurnal_rezki',{id}).then(setJournal).catch(()=>{}),1000);return()=>clearInterval(timer);},[!!journal,id]);
 useEffect(()=>{const root=document.documentElement;root.dataset.tema=settings.tema;root.dataset.plotnost=settings.plotnost;root.style.setProperty('--font-ui',shrifty[settings.shrift]||shrifty.system);root.style.setProperty('--font-size-ui',settings.razmer_shrifta+'px');},[settings]);
 return <div className="secondary-window" onMouseDownCapture={e=>{if(e.button===0&&!editableTarget(e.target))void invoke('fokus_obolochke').catch(()=>{});}} onContextMenu={e=>{if(!editableTarget(e.target))e.preventDefault();}}>
  <ObychnyeVkladki id={id} nazvanie={name} novayaStranica={settings.novaya_stranica} poisk={settings.poisk} poleKontenta={content}><button className="schet" title="Супер-вкладки и настройки" aria-label="Супер-вкладки и настройки" onClick={()=>void invoke('pokazat_glavnoe').catch(e=>setError(String(e)))}>☰</button><button className="schet protection-button" aria-label="Защита супер-вкладки" onClick={async()=>{await invoke('pokazat_dvizhki',{vidno:false});try{setJournal(await invoke<Zhurnal>('zhurnal_rezki',{id}));}catch(e){setError(String(e));await invoke('pokazat_dvizhki',{vidno:true});}}}>Защита</button></ObychnyeVkladki>
  {error&&<p role="alert">{error}</p>}
  <div className="pole" ref={content}/>
  <Biblioteka superId={id}/>
  {journal&&<Blokirovki journal={journal} name={name} onClose={()=>{setJournal(null);void invoke('pokazat_dvizhki',{vidno:true});}}/>}
 </div>;
}
