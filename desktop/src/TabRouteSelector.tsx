import {useCallback,useEffect,useRef,useState} from 'react';
import {createPortal} from 'react-dom';
import {invoke} from '@tauri-apps/api/core';
import {listenWindow} from './windowEvents';
import Icon from './Icons';
import Vybor from './Vybor';
import {checkedRouteChoices,routeFromKey,routeKey,routeLabel,routeStateKnown,type SelectableTabRoute,type TabRouteChoices,type TabRouteTarget} from './tabRouteTypes';
import './tabRoutes.css';

// Serialize this overlay's native visibility requests, including late hides.
let visibilityEpoch=0;
let visibilityQueue:Promise<unknown>=Promise.resolve();
function routeVisibility(visible:boolean){
 const result=visibilityQueue.catch(()=>{}).then(()=>invoke('pokazat_dvizhki',{vidno:visible}));
 visibilityQueue=result.catch(()=>{});return result;
}
const modalSelector='.zatemnenie,[role=dialog],[role=alertdialog],.browser-lock-screen,.settings-overlay,.security-overlay';

export function TabRouteButton({tab,choices,onOpen}:{tab:TabRouteTarget|undefined;choices:TabRouteChoices|null;onOpen:()=>void}){
 const known=!!tab&&routeStateKnown(tab);
 const state=tab?.route_pending?'pending':tab?.route_error||tab?.route?.kind==='blocked'?'error':known?'applied':'unknown';
 const label=routeLabel(tab?.route,choices);
 const detail=state==='pending'?'Применяем путь. Подтверждённый: '+label:tab?.route_error?'Путь не применён. '+tab.route_error+' Подтверждённый: '+label:label;
 return <button type="button" className="tab-route-button" data-route-state={state} aria-label="Путь вкладки" aria-busy={!!tab?.route_pending} title={'Путь вкладки: '+detail} disabled={!known||!!tab?.memory_pending||!!tab?.route_pending} onClick={onOpen}>
  <Icon name="globe" size={17}/><i aria-hidden="true"/>
 </button>;
}

export function useTabRouteChoices(owner:string|null,generation:number|undefined){
 const [choices,setChoices]=useState<TabRouteChoices|null>(null),[error,setError]=useState('');
 const request=useRef(0),live=useRef(false),ownerRef=useRef(owner);ownerRef.current=owner;
 const refresh=useCallback(async()=>{
  if(!owner||ownerRef.current!==owner)return;
  const token=++request.current;setError('');
  try{const result=checkedRouteChoices(await invoke<TabRouteChoices>('tab_route_choices',{id:owner}));if(live.current&&ownerRef.current===owner&&token===request.current)setChoices(result);}
  catch(value){if(live.current&&ownerRef.current===owner&&token===request.current){setChoices(null);setError(String(value));}}
 },[owner]);
 useEffect(()=>{
  live.current=true;setChoices(null);setError('');
  if(owner)void refresh();
  const off=listenWindow('nastrojki-izmenilis',()=>{void refresh();});
  return()=>{live.current=false;request.current++;void off.then(stop=>stop());};
 },[owner,generation,refresh]);
 return {choices,error,refresh};
}

export function TabRouteDialog({owner,tab,choices,choicesError,onRefresh,onApply,onClose,onError}:{
 owner:string;tab:TabRouteTarget;choices:TabRouteChoices|null;choicesError:string;onRefresh:()=>void;
 onApply:(route:SelectableTabRoute,expectedGeneration:number)=>Promise<void>;onClose:()=>void;onError:(message:string)=>void;
}){
 const [ready,setReady]=useState(false),[draft,setDraft]=useState(routeKey(tab.route));
 const [applying,setApplying]=useState(false),[error,setError]=useState(''),[notice,setNotice]=useState('');
 const running=useRef(false),alive=useRef(true),cancel=useRef<HTMLButtonElement>(null),dialog=useRef<HTMLElement>(null);
 const callbacks=useRef({onClose,onError});callbacks.current={onClose,onError};
 const previousGeneration=useRef(tab.route_generation);
 const busy=applying||!!tab.route_pending;
 useEffect(()=>{
  if(previousGeneration.current!==tab.route_generation){
   previousGeneration.current=tab.route_generation;
   if(!running.current){setDraft(routeKey(tab.route));setNotice('Путь изменён. Проверьте выбор перед применением.');}
  }
 },[tab.route,tab.route_generation]);
 useEffect(()=>{
  alive.current=true;
  const otherModal=()=>Array.from(document.querySelectorAll(modalSelector)).some(element=>!element.closest('[data-tab-route-root]'));
  if(otherModal()){callbacks.current.onClose();return;}
  ++visibilityEpoch;
  const hidden=routeVisibility(false);
  void hidden.then(()=>{if(!alive.current)return;if(otherModal()){callbacks.current.onClose();return;}setReady(true);}).catch(value=>{if(alive.current){callbacks.current.onError(String(value));callbacks.current.onClose();}});
  const observer=new MutationObserver(()=>{if(otherModal())callbacks.current.onClose();});observer.observe(document.body,{childList:true,subtree:true});
  return()=>{
   alive.current=false;observer.disconnect();const epoch=++visibilityEpoch;
   void hidden.catch(()=>{}).then(()=>requestAnimationFrame(()=>{
    if(epoch!==visibilityEpoch||document.querySelector(modalSelector))return;
    void routeVisibility(true).then(()=>{if(document.querySelector(modalSelector))void routeVisibility(false).catch(()=>{});}).catch(()=>{});
   }));
  };
 },[owner,tab.id]);
 useEffect(()=>{if(ready)cancel.current?.focus();},[ready]);
 const chosen=routeFromKey(draft);
 const choiceAvailable=!!chosen&&(chosen.kind!=='connection'||!!choices?.connections.some(value=>value.id===chosen.id));
 async function apply(){
  if(running.current||busy||!choiceAvailable||!chosen||!routeStateKnown(tab)||(draft===routeKey(tab.route)&&!tab.route_error))return;
  running.current=true;setApplying(true);setError('');setNotice('');
  try{await onApply(chosen,tab.route_generation!);if(alive.current)callbacks.current.onClose();}
  catch(value){if(alive.current)setError(String(value));}
  finally{running.current=false;if(alive.current)setApplying(false);}
 }
 if(!ready)return null;
 const missing=draft.startsWith('connection:')&&!choices?.connections.some(value=>'connection:'+value.id===draft);
 return createPortal(<div className="zatemnenie" data-tab-route-root="true" onClick={()=>{if(!busy)onClose();}}>
  <section className="dialog tab-route-dialog" ref={dialog} tabIndex={-1} role="dialog" aria-modal="true" aria-label="Путь вкладки" aria-describedby="tab-route-profile-warning" onClick={event=>event.stopPropagation()} onKeyDown={event=>{
   if(event.key==='Escape'){event.preventDefault();event.stopPropagation();if(!busy)onClose();}
   if(event.key==='Tab'){
    const controls=Array.from(event.currentTarget.querySelectorAll<HTMLElement>('button:not(:disabled),input:not(:disabled),[tabindex="0"]'));
    event.preventDefault();
    const insidePopup=(event.target as Element).closest('.vybor-popup');
    const current=insidePopup?controls.findIndex(element=>element.getAttribute('role')==='combobox'):controls.indexOf(document.activeElement as HTMLElement);
    if(controls.length)controls[(current+(event.shiftKey?-1:1)+controls.length)%controls.length]?.focus();else dialog.current?.focus();
   }
  }}>
   <header><h2>Путь вкладки</h2><button type="button" aria-label="Закрыть выбор пути вкладки" disabled={busy} onClick={onClose}><Icon name="close" size={17}/></button></header>
   <strong className="tab-route-tab-title">{tab.zagolovok||'Новая вкладка'}</strong>
   <p className="tab-route-current">Текущий путь: {routeStateKnown(tab)?routeLabel(tab.route,choices):'ещё не получен'}</p>
   <label className="tab-route-choice">Подключение<Vybor aria-label="Выберите путь вкладки" value={draft} disabled={busy||!choices} onChange={event=>{setDraft(event.target.value);setError('');setNotice('');}}>
    {draft==='blocked'&&<option value="blocked" disabled>Сохранённый путь недоступен — выберите другой</option>}
    {missing&&<option value={draft} disabled>Выбранное подключение недоступно</option>}
    <option value="inherit">{choices?'Как у супервкладки: '+choices.inheritLabel:'Как у супервкладки'}</option>
    <option value="direct">Прямое соединение</option>
    {choices?.connections.map(value=><option key={value.id} value={'connection:'+value.id}>{value.name}</option>)}
   </Vybor></label>
   <p id="tab-route-profile-warning" className="tab-route-warning">Страница перезагрузится. Может понадобиться повторный вход.</p>
   {!choices&&!choicesError&&<p role="status">Загружаю сохранённые подключения…</p>}
   {choicesError&&<div className="tab-route-error" role="alert"><span>{choicesError}</span><button type="button" disabled={busy} onClick={onRefresh}>Повторить получение подключений</button></div>}
   {(error||tab.route_error)&&<p className="tab-route-error" role="alert">{error||tab.route_error}</p>}
   {notice&&<p role="status">{notice}</p>}
   {busy&&<p role="status" aria-live="polite">Меняем путь…</p>}
   <div className="knopki"><button ref={cancel} type="button" disabled={busy} onClick={onClose}>Отмена</button><button type="button" className="glavnaya" disabled={busy||!choices||!choiceAvailable||!routeStateKnown(tab)||(draft===routeKey(tab.route)&&!tab.route_error)} onClick={()=>void apply()}>{busy?'Применяем…':'Применить путь'}</button></div>
  </section>
 </div>,document.body);
}
