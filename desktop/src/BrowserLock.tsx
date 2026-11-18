import {useCallback,useEffect,useRef,useState,type FormEvent,type ReactNode} from 'react';
import {invoke} from '@tauri-apps/api/core';
import {getCurrentWindow} from '@tauri-apps/api/window';
import {shortcutKey} from './shortcuts';
import './BrowserLock.css';

type LockStatus={locked:boolean;exists:boolean;revision:number};
export default function BrowserGuard({children}:{children:ReactNode}){
 const [status,setStatus]=useState<LockStatus|null>(null),[error,setError]=useState(''),[password,setPassword]=useState(''),[busy,setBusy]=useState(false),[hint,setHint]=useState('');
 const alive=useRef(true),request=useRef(false);
 const apply=useCallback((next:LockStatus)=>{if(!alive.current)return;setStatus(previous=>!previous||next.revision>=previous.revision?next:previous);},[]);
 const refresh=useCallback(async()=>{const next=await invoke<LockStatus>('browser_lock_status');apply(next);},[apply]);
 useEffect(()=>{
  alive.current=true;
  const off=getCurrentWindow().listen<LockStatus>('browser-lock-changed',event=>apply(event.payload));
  const unavailable=getCurrentWindow().listen<string>('browser-lock-unavailable',event=>{if(alive.current){setHint(event.payload);window.dispatchEvent(new Event('nexusbro-security'));}});
  void Promise.all([off,unavailable]).then(()=>refresh()).catch(e=>{if(alive.current)setError(String(e));});
  const timer=setInterval(()=>{void refresh().catch(()=>{});},2000);
  return()=>{alive.current=false;clearInterval(timer);void off.then(stop=>stop());void unavailable.then(stop=>stop());};
 },[apply,refresh]);
 useEffect(()=>{setPassword('');if(status?.locked){setHint('');setError('');}},[status?.locked]);
 useEffect(()=>{
  const key=(event:KeyboardEvent)=>{
   if(!event.ctrlKey||!event.shiftKey||event.altKey||event.metaKey||shortcutKey(event)!=='l')return;
   event.preventDefault();event.stopImmediatePropagation();
   if(status?.locked||request.current)return;
   request.current=true;
   void invoke<LockStatus>('browser_lock').then(apply).catch(e=>{if(alive.current){setHint(String(e));window.dispatchEvent(new Event('nexusbro-security'));}}).finally(()=>{request.current=false;});
  };
  window.addEventListener('keydown',key,true);return()=>window.removeEventListener('keydown',key,true);
 },[status?.locked,apply]);
 async function unlock(event:FormEvent){
  event.preventDefault();if(request.current)return;request.current=true;setBusy(true);setError('');const secret=password;setPassword('');
  try{apply(await invoke<LockStatus>('browser_unlock',{password:secret}));}catch(e){if(alive.current)setError(String(e));}
  finally{request.current=false;if(alive.current)setBusy(false);}
 }
 if(!status||status.locked)return <main className="browser-lock-screen" aria-label="Браузер заблокирован">
  <section className="browser-lock-card">
   <img className="browser-lock-icon" src="/nexusbro.png" alt=""/>
   <h1>NexusBro</h1>
   {!status?<><p>Проверяем защиту браузера…</p>{error&&<><p className="browser-lock-error" role="alert">Не удалось проверить блокировку. Открытые сайты скрыты.</p><button type="button" onClick={()=>{setError('');void refresh().catch(e=>setError(String(e)));}}>Повторить</button></>}</>:<>
    <h2>Браузер заблокирован</h2>
    <p>Введите мастер-пароль хранилища, чтобы открыть вкладки и сохранённые пароли.</p>
    <form onSubmit={unlock}>
     <label>Мастер-пароль<input autoFocus aria-label="Мастер-пароль для разблокировки браузера" type="password" autoComplete="current-password" value={password} onChange={event=>setPassword(event.target.value)} required disabled={busy}/></label>
     {error&&<p className="browser-lock-error" role="alert">{error}</p>}
     <button type="submit" disabled={busy||!password}>{busy?'Проверяем…':'Разблокировать браузер'}</button>
    </form>
    <p className="browser-lock-note">Это локальный мастер-пароль, а не пароль аккаунта NexusBro. Блокировка: Ctrl+Shift+L.</p>
   </>}
  </section>
 </main>;
 return <>{children}{hint&&<aside className="browser-lock-hint" role="status"><span>{hint}</span><button type="button" onClick={()=>{setHint('');window.dispatchEvent(new Event('nexusbro-security'));}}>Настроить защиту</button><button type="button" aria-label="Закрыть подсказку блокировки" onClick={()=>setHint('')}>×</button></aside>}</>;
}
