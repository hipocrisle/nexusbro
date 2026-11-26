import {useEffect,useRef,useState} from 'react';
import {invoke} from '@tauri-apps/api/core';
import {listenWindow as listen} from './windowEvents';
import './downloadsButton.css';

type Download={id:string;super_id:string;privatnaya:boolean;status:string;percent:number;received:number;total:number};

/** Компактная кнопка загрузок рядом с адресной строкой (вставка — за ROOT,
 * рядом с TabRouteButton в ObychnyeVkladki.tsx).
 *
 * Обновление: mount + событие-инвалидация `downloads-changed` (без содержимого)
 * → invoke zagruzki_spisok с точным фильтром super_id+privatnaya. Никакого
 * poll-таймера в idle; throttle не чаще 4 Гц; listeners/timers очищаются,
 * устаревшие ответы отсекаются поколением.
 *
 * Приватность: в badge/tooltip только количества и процент — никаких имён,
 * URL, путей. Приватные загрузки не видны обычным вкладкам (точное
 * совпадение флага privatnaya).
 *
 * DEPENDENCY: backend-инвалидация `downloads-changed` после обновления
 * хранилища добавляется ROOT (lib.rs — его файл). До появления события кнопка
 * честно обновляется на mount и по событиям библиотеки; «живой» статус не
 * обещается. */
export default function DownloadsButton({superId,privatnaya,onOpen}:{superId:string|null;privatnaya:boolean;onOpen:()=>void}){
 const [items,setItems]=useState<Download[]>([]);
 const generation=useRef(0),pending=useRef<number|undefined>(undefined),lastRun=useRef(0);
 function refresh(){
  const gen=++generation.current;
  invoke<Download[]>('zagruzki_spisok').then(list=>{
   if(generation.current!==gen)return;
   setItems(list.filter(d=>d.super_id===superId&&d.privatnaya===privatnaya));
  }).catch(()=>{if(generation.current===gen)setItems([]);});
 }
 function schedule(){
  if(pending.current!==undefined)return;
  const wait=Math.max(0,250-(Date.now()-lastRun.current));
  pending.current=window.setTimeout(()=>{pending.current=undefined;lastRun.current=Date.now();refresh();},wait);
 }
 useEffect(()=>{
  if(!superId){setItems([]);return;}
  lastRun.current=0;refresh();
  const off=listen<unknown>('downloads-changed',()=>schedule());
  return()=>{generation.current++;if(pending.current!==undefined){clearTimeout(pending.current);pending.current=undefined;}void off.then(f=>f());};
 },[superId,privatnaya]);
 const active=items.filter(d=>d.status==='Загружается'||d.status==='Приостановлена');
 const known=active.length>0&&active.every(d=>d.total>0&&d.percent>=0);
 const weighted=known?Math.round(active.reduce((sum,d)=>sum+d.received,0)/active.reduce((sum,d)=>sum+d.total,0)*100):null;
 const paused=active.length>0&&active.every(d=>d.status==='Приостановлена');
 const title=active.length
  ?`Загрузки: активных ${active.length}${weighted!==null?` · ${weighted}%`:' · размер неизвестен'}${paused?' · пауза':''}`
  :'Загрузки';
 return <button type="button" className={'downloads-button'+(active.length?' active':'')+(paused?' paused':'')}
  title={title} aria-label={title} onClick={onOpen}>
  <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.65" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
   <path d="M12 4v10"/><path d="m8 11 4 4 4-4"/><path d="M5 19h14"/>
  </svg>
  {active.length>0&&<span className="downloads-badge" aria-hidden="true">{active.length}</span>}
  {active.length>0&&<span className={'downloads-progress'+(weighted===null?' indeterminate':'')}
   style={weighted!==null?{width:Math.max(4,Math.min(100,weighted))+'%'}:undefined} aria-hidden="true"/>}
 </button>;
}
