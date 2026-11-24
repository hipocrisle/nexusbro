import {useCallback,useEffect,useRef,useState,type FormEvent,type ReactNode} from 'react';
import {invoke} from '@tauri-apps/api/core';
import {getCurrentWindow} from '@tauri-apps/api/window';
import {shortcutKey} from './shortcuts';
import Vybor from './Vybor';
import './BrowserLock.css';

type VaultBackup={id:string;created:number};
type LockStatus={locked:boolean;exists:boolean;revision:number};
export default function BrowserGuard({children}:{children:ReactNode}){
 const [status,setStatus]=useState<LockStatus|null>(null),[error,setError]=useState(''),[password,setPassword]=useState(''),[busy,setBusy]=useState(false),[hint,setHint]=useState('');
 const [mode,setMode]=useState<'unlock'|'reset'|'restore'>('unlock'),[newPassword,setNewPassword]=useState(''),[repeat,setRepeat]=useState(''),[confirmation,setConfirmation]=useState(''),[backups,setBackups]=useState<VaultBackup[]>([]),[backupId,setBackupId]=useState('');
 const alive=useRef(true),unlocking=useRef(false),recoveryQuery=useRef(0);
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
 useEffect(()=>{recoveryQuery.current++;setPassword('');setNewPassword('');setRepeat('');setConfirmation('');setMode('unlock');setBackups([]);setBackupId('');if(status?.locked){setHint('');setError('');}},[status?.locked]);
 useEffect(()=>{
  const key=(event:KeyboardEvent)=>{
   if(!event.ctrlKey||!event.shiftKey||event.altKey||event.metaKey||shortcutKey(event)!=='l')return;
   event.preventDefault();event.stopImmediatePropagation();
   // Native lock/unlock operations are serialized. Never discard a fresh
   // lock intent because an earlier unlock response has not arrived yet.
   // Ignore held-key repeats, but forward every distinct press, including
   // while locally locked: an unlock may already be queued in native code.
   if(event.repeat)return;
   void invoke<LockStatus>('browser_lock').then(apply).catch(e=>{if(alive.current){setHint(String(e));window.dispatchEvent(new Event('nexusbro-security'));}});
  };
  window.addEventListener('keydown',key,true);return()=>window.removeEventListener('keydown',key,true);
 },[apply]);
 function changeMode(next:'unlock'|'reset'|'restore'){
  if(unlocking.current)return;
  recoveryQuery.current++;setMode(next);setError('');setPassword('');setNewPassword('');setRepeat('');setConfirmation('');
  if(next==='restore')void loadBackups();
 }
 async function loadBackups(){
  if(unlocking.current)return;
  const query=++recoveryQuery.current;unlocking.current=true;setBusy(true);setError('');setBackups([]);setBackupId('');
  try{const copies=await invoke<VaultBackup[]>('browser_vault_backups');if(alive.current&&query===recoveryQuery.current){setBackups(copies);setBackupId(copies[0]?.id??'');}}
  catch(e){if(alive.current&&query===recoveryQuery.current)setError(String(e));}
  finally{unlocking.current=false;if(alive.current)setBusy(false);}
 }
 async function submit(event:FormEvent){
  event.preventDefault();if(unlocking.current)return;
  if(mode==='reset'&&(newPassword.length<12||newPassword!==repeat||confirmation!=='СБРОСИТЬ')){setError('Укажите совпадающие пароли от 12 символов и точное подтверждение СБРОСИТЬ.');return;}
  if(mode==='restore'&&(!backupId||!password))return;
  unlocking.current=true;setBusy(true);setError('');const secret=mode==='reset'?newPassword:password;
  setPassword('');setNewPassword('');setRepeat('');setConfirmation('');
  try{
   const next=mode==='reset'?await invoke<LockStatus>('browser_vault_reset',{password:secret,confirmation}):mode==='restore'?await invoke<LockStatus>('browser_vault_restore',{id:backupId,password:secret}):await invoke<LockStatus>('browser_unlock',{password:secret});
   apply(next);if(alive.current){setMode('unlock');setBackups([]);setBackupId('');}
  }catch(e){if(alive.current)setError(String(e));}
  finally{unlocking.current=false;if(alive.current)setBusy(false);}
 }
 function copyLabel(copy:VaultBackup,index:number){const date=new Date(copy.created<1e12?copy.created*1000:copy.created);return `${Number.isNaN(date.getTime())?'Дата неизвестна':date.toLocaleString('ru-RU')} · копия ${index+1}`;}
 if(!status||status.locked)return <main className="browser-lock-screen" aria-label="Браузер заблокирован">
  <section className={"browser-lock-card"+(mode!=='unlock'?' browser-lock-recovery':'')}>
   <img className="browser-lock-icon" src="/nexusbro.png" alt=""/>
   <h1>NexusBro</h1>
   {!status?<><p>Проверяем защиту браузера…</p>{error&&<><p className="browser-lock-error" role="alert">Не удалось проверить блокировку. Открытые сайты скрыты.</p><button type="button" onClick={()=>{setError('');void refresh().catch(e=>setError(String(e)));}}>Повторить</button></>}</>:<>
    <h2>{mode==='reset'?'Сброс хранилища':mode==='restore'?'Восстановление из копии':'Браузер заблокирован'}</h2>
    {mode==='unlock'?<p>Введите мастер-пароль хранилища, чтобы открыть вкладки и сохранённые пароли.</p>:mode==='reset'?<div className="browser-lock-warning"><p>Забытый мастер-пароль невозможно восстановить. Будет создано пустое хранилище, а прежнее сохранится в зашифрованной копии.</p><p>Сохранённые пароли, локальный доступ к аккаунту и защищённая история останутся в старой копии. Для её восстановления понадобится прежний мастер-пароль.</p><p>Сайты, cookies, вкладки и обычные настройки останутся. Данные в облаке не удаляются и не перезаписываются автоматически.</p></div>:<p>Выберите копию и введите её прежний мастер-пароль. Текущее хранилище будет сохранено в отдельной зашифрованной копии. Данные в облаке не изменятся.</p>}
    <form onSubmit={submit} noValidate>
     {mode==='reset'?<>
      <label>Новый мастер-пароль<input autoFocus aria-label="Новый мастер-пароль" type="password" autoComplete="new-password" value={newPassword} onChange={event=>setNewPassword(event.target.value)} disabled={busy}/></label>
      <small>Не менее 12 символов. Сохраните пароль в надёжном месте.</small>
      <label>Повторите новый мастер-пароль<input aria-label="Повторите новый мастер-пароль" type="password" autoComplete="new-password" value={repeat} onChange={event=>setRepeat(event.target.value)} disabled={busy}/></label>
      <label>Подтверждение сброса<input aria-label="Подтверждение сброса" autoComplete="off" spellCheck={false} placeholder="Введите СБРОСИТЬ" value={confirmation} onChange={event=>setConfirmation(event.target.value)} disabled={busy}/></label>
     </>:<>
      {mode==='restore'&&<>{backups.length?<label>Копия хранилища<Vybor aria-label="Копия хранилища" value={backupId} disabled={busy} onChange={event=>setBackupId(event.target.value)}>{backups.map((copy,index)=><option key={copy.id} value={copy.id}>{copyLabel(copy,index)}</option>)}</Vybor></label>:<p className="browser-lock-note">{busy?'Ищем сохранённые копии…':'Сохранённых копий не найдено.'}</p>}</>}
      {(mode==='unlock'||backups.length>0)&&<label>{mode==='restore'?'Мастер-пароль выбранной копии':'Мастер-пароль'}<input autoFocus aria-label={mode==='restore'?'Мастер-пароль выбранной копии':'Мастер-пароль для разблокировки браузера'} type="password" autoComplete="current-password" value={password} onChange={event=>setPassword(event.target.value)} disabled={busy}/></label>}
     </>}
     {error&&<p className="browser-lock-error" role="alert">{error}</p>}
     {(mode!=='restore'||backups.length>0)&&<button className={mode==='reset'?'browser-lock-danger':''} type="submit" disabled={busy||(mode==='reset'?(newPassword.length<12||newPassword!==repeat||confirmation!=='СБРОСИТЬ'):!password)}>{busy?'Проверяем…':mode==='reset'?'Сбросить хранилище':mode==='restore'?'Восстановить хранилище':'Разблокировать браузер'}</button>}
    </form>
    {mode==='unlock'?<>
     <p className="browser-lock-note">Это локальный мастер-пароль, а не пароль аккаунта NexusBro. Блокировка: Ctrl+Shift+L.</p>
     {status.exists&&<div className="browser-lock-actions"><button className="browser-lock-secondary" type="button" disabled={busy} onClick={()=>changeMode('reset')}>Забыли мастер-пароль?</button><button className="browser-lock-secondary" type="button" disabled={busy} onClick={()=>changeMode('restore')}>Восстановить из копии</button></div>}
    </>:<div className="browser-lock-actions"><button className="browser-lock-secondary" type="button" disabled={busy} onClick={()=>changeMode('unlock')}>Назад</button>{mode==='reset'?<button className="browser-lock-secondary" type="button" disabled={busy} onClick={()=>changeMode('restore')}>Восстановить из копии</button>:<button className="browser-lock-secondary" type="button" disabled={busy} onClick={()=>void loadBackups()}>Обновить список копий</button>}</div>}
   </>}
  </section>
 </main>;
 return <>{children}{hint&&<aside className="browser-lock-hint" role="status"><span>{hint}</span><button type="button" onClick={()=>{setHint('');window.dispatchEvent(new Event('nexusbro-security'));}}>Настроить защиту</button><button type="button" aria-label="Закрыть подсказку блокировки" onClick={()=>setHint('')}>×</button></aside>}</>;
}
