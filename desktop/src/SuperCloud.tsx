import {useCallback,useEffect,useRef,useState} from 'react';
import {invoke} from '@tauri-apps/api/core';
import {listenWindow} from './windowEvents';
import {SECURITY_STATE_CHANGED,type AccountStatus,type SyncStatus,type VaultStatus} from './security';
import './superCloud.css';

export type SuperCloudState='local'|'pending'|'selected'|'conflict'|'paused'|'error';
type Change={area?:string;action?:string;failed?:boolean;phase?:'start'|'end';opId?:string};
type Snapshot={account:AccountStatus;sync:SyncStatus;paused:boolean;unavailable:boolean};
const ordinary=(id:string,privateFlag?:boolean)=>!privateFlag&&!id.startsWith('pvt-');
function stamp(value:SyncStatus['last_sync']){const n=typeof value==='number'&&value<100000000000?value*1000:value;if(!n)return null;const date=new Date(n);return Number.isNaN(date.getTime())?null:date;}
export function accountSyncReceipt(value:SyncStatus['last_sync']){const date=stamp(value);return date?'Последняя успешная синхронизация аккаунта: '+date.toLocaleString('ru-RU'):'Успешных синхронизаций аккаунта ещё не было';}
export function CloudGlyph({state='selected'}:{state?:SuperCloudState}){
 return <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.65" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M6.5 18a4.5 4.5 0 1 1 .6-9A6 6 0 0 1 18.5 8a5 5 0 0 1-.5 10H6.5Z"/>{state==='paused'?<path d="m4 3 16 18"/>:state==='conflict'||state==='error'?<><path d="M12 10v4"/><circle cx="12" cy="17" r=".7" fill="currentColor" stroke="none"/></>:state==='pending'?<><circle cx="9" cy="13" r=".8" fill="currentColor" stroke="none"/><circle cx="12" cy="13" r=".8" fill="currentColor" stroke="none"/><circle cx="15" cy="13" r=".8" fill="currentColor" stroke="none"/></>:null}</svg>;
}
export function SuperCloudBadge({state,label}:{state:SuperCloudState;label:string}){
 return <span className="super-cloud-badge" data-cloud-state={state} title={label} aria-label={label}><CloudGlyph state={state}/></span>;
}
export function SuperCloudHeading({cloud,receipt,state='local'}:{cloud:boolean;receipt?:SyncStatus['last_sync'];state?:SuperCloudState}){
 const label=cloud?'Облако':'На устройстве';
 const status=state==='selected'?'Выбранные данные подтверждены сервером':state==='error'||state==='conflict'?'Ошибка синхронизации':state==='local'?'Синхронизация выключена':'Ожидает подтверждения синхронизации';
 const description=cloud?label+'. '+status+'. '+accountSyncReceipt(receipt):label;
 return <header className="super-cloud-heading" data-cloud-state={state} title={description} aria-label={description} {...(cloud?{'data-testid':'cloud-last-account-sync','data-last-account-sync':receipt??'','data-last-sync':receipt??''}:{})}><span className="super-cloud-heading-icon">{cloud?<CloudGlyph state={state}/>:<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.65" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><rect x="4" y="4" width="16" height="12" rx="2"/><path d="M8 20h8m-4-4v4"/></svg>}</span><span className="super-cloud-heading-text"><strong>{label}</strong></span></header>;
}

// No protected-status polling: vault reads are activity, so polling would prevent idle lock.
export function useSuperCloud(securityOpen:boolean){
 const [snapshot,setSnapshot]=useState<Snapshot|null>(null);
 const [busy,setBusy]=useState(false);const operations=useRef(new Set<string>()),completed=useRef(new Set<string>());
 const retained=useRef<Snapshot|null>(null),generation=useRef(0),alive=useRef(false),timer=useRef<ReturnType<typeof setTimeout>|null>(null);
 const refresh=useCallback(()=>{
  const token=++generation.current;if(timer.current)clearTimeout(timer.current);
  timer.current=setTimeout(()=>{timer.current=null;void (async()=>{
   try{
    const vault=await invoke<VaultStatus>('security_vault',{action:'status'});if(!alive.current||token!==generation.current)return;
    if(!vault?.unlocked){retained.current=null;setSnapshot(null);return;}
    const account=await invoke<AccountStatus>('security_account',{action:'status'});if(!alive.current||token!==generation.current)return;
    if(!account?.configured||!account.username){retained.current=null;setSnapshot(null);return;}
    const previous=retained.current?.account;if(previous&&(previous.server!==account.server||previous.username!==account.username||(previous.user_id&&account.user_id&&previous.user_id!==account.user_id))){retained.current=null;setSnapshot(null);}
    if(!account.logged_in){const old=retained.current;if(old?.account.server===account.server&&old.account.username===account.username){const next={...old,account,paused:true};retained.current=next;setSnapshot(next);}else{retained.current=null;setSnapshot(null);}return;}
    const status=await invoke<SyncStatus>('security_sync',{action:'status'});if(!alive.current||token!==generation.current)return;
    if(!status?.selection||!Array.isArray(status.selection.supers))throw Error('Sync metadata unavailable');
    const next={account,sync:status,paused:false,unavailable:false};retained.current=next;setSnapshot(next);
   }catch{if(alive.current&&token===generation.current){const old=retained.current;setSnapshot(old?{...old,paused:true,unavailable:true}:null);}}
  })();},60);
 },[]);
 useEffect(()=>{
  alive.current=true;
  const changed=(event:Event)=>{const detail=(event as CustomEvent<Change>).detail;if(detail?.area==='sync'&&detail.action==='run'){
    if(detail.opId&&detail.phase==='start'){if(!completed.current.has(detail.opId))operations.current.add(detail.opId);}
    else if(detail.opId&&detail.phase==='end'){operations.current.delete(detail.opId);completed.current.add(detail.opId);if(completed.current.size>128)completed.current.delete(completed.current.values().next().value!);}
    setBusy(operations.current.size>0);
   }refresh();};
  const focus=()=>refresh();const visible=()=>{if(!document.hidden)refresh();};
  window.addEventListener(SECURITY_STATE_CHANGED,changed);window.addEventListener('focus',focus);document.addEventListener('visibilitychange',visible);
  const off=listenWindow<Change>(SECURITY_STATE_CHANGED,event=>changed(new CustomEvent(SECURITY_STATE_CHANGED,{detail:event.payload})));
  refresh();return()=>{alive.current=false;generation.current++;if(timer.current)clearTimeout(timer.current);window.removeEventListener(SECURITY_STATE_CHANGED,changed);window.removeEventListener('focus',focus);document.removeEventListener('visibilitychange',visible);void off.then(stop=>stop()).catch(()=>{});};
 },[refresh]);
 useEffect(()=>{if(!securityOpen)refresh();},[securityOpen,refresh]);
 const selected=(id:string,privateFlag?:boolean)=>ordinary(id,privateFlag)&&!!snapshot?.account.logged_in&&!!snapshot?.sync.selection.supers.some(key=>key==='*'||key===id);
 const state=(id:string,privateFlag?:boolean):SuperCloudState=>{
  if(!selected(id,privateFlag))return 'local';
  if(snapshot?.sync.super_states?.[id]?.state==='conflict'||snapshot?.sync.report?.conflicts?.includes('nexusbro.v1/super/'+id))return 'conflict';
  if(snapshot?.sync.super_states?.[id]?.state==='error'||snapshot?.sync.report?.errors?.length)return 'error';
  if(snapshot?.paused)return 'paused';
  if(busy)return 'pending';
  return snapshot?.sync.super_states?.[id]?.state==='ok'?'selected':'pending';
 };
 const label=(id:string,privateFlag?:boolean)=>{
  const kind=state(id,privateFlag);
  if(kind==='local')return privateFlag||id.startsWith('pvt-')?'Приватная супер-вкладка · не синхронизируется':'Только на этом устройстве';
  if(kind==='paused')return 'Выбрана для синхронизации. '+(snapshot?.unavailable?'Не удалось проверить состояние аккаунта.':'Войдите в аккаунт, чтобы продолжить синхронизацию.');
  if(kind==='error')return 'Не удалось синхронизировать. Откройте настройки синхронизации.';
  if(kind==='conflict')return 'Выбрана для синхронизации. Конфликт версий — откройте настройки синхронизации.';
  if(kind==='pending')return 'Выбрана для синхронизации. Ожидает успешного завершения синхронизации аккаунта.';
  return 'Синхронизирована: текущая версия подтверждена сервером.';
 };
 const overall:SuperCloudState=!snapshot?.account.logged_in?'local':snapshot?.sync.report?.errors?.length||snapshot?.sync.report?.conflicts?.length||snapshot?.sync.sync_state?.state==='error'?'error':snapshot.paused?'paused':busy?'pending':snapshot.sync.sync_state?.state==='ok'?'selected':snapshot.sync.sync_state?.state==='off'?'local':'pending';
 return {selected,state,label,refresh,lastSync:snapshot?.sync.last_sync,overall};
}
