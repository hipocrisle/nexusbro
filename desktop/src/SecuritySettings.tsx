import {useEffect,useRef,useState,type FormEvent,type ReactNode} from 'react';
import {invoke} from '@tauri-apps/api/core';
import Icon from './Icons';
import Vybor from './Vybor';
import {security,currentCredentialTarget,emptySelection,type VaultStatus,type CredentialSummary,type CredentialDraft,type AccountStatus,type SyncStatus,type SyncReport,type SyncSelection} from './security';
import './securitySettings.css';

type Tab='passwords'|'account'|'sync';
const DEFAULT_ACCOUNT_SERVER='https://nexusbro-sync.hipogas.org';
type AccountResult=AccountStatus&{recovery_key?:string;recovery_codes?:string[];warning?:string};
type Enrollment={secret:string;otpauth_uri:string};
function errorText(e:unknown){
 const message=String(e instanceof Error?e.message:e).slice(0,500);
 if(/totp[ _-]?required/i.test(message))return 'Введите код двухфакторной аутентификации или резервный код.';
 if(/invalid credentials/i.test(message))return 'Неверное имя аккаунта, пароль или код 2FA.';
 if(/vault locked/i.test(message))return 'Хранилище заблокировано. Разблокируйте его мастер-паролем.';
 return message;
}
function dateText(value:number|string|null|undefined){if(!value)return 'Ещё не выполнялась';const n=typeof value==='number'&&value<100000000000?value*1000:value;const d=new Date(n);return Number.isNaN(d.getTime())?'Дата недоступна':d.toLocaleString('ru-RU');}
function Secret({value,onChange,label,autoComplete='off',minLength,required=true}:{value:string;onChange:(v:string)=>void;label:string;autoComplete?:string;minLength?:number;required?:boolean}){
 return <label>{label}<input type="password" aria-label={label} value={value} onChange={e=>onChange(e.target.value)} autoComplete={autoComplete} minLength={minLength} required={required}/></label>;
}
function toggleId(ids:string[],id:string){return ids.includes(id)?ids.filter(v=>v!==id):[...ids,id];}
export default function SecuritySettings({onClose}:{onClose:()=>void}){
 const [tab,setTab]=useState<Tab>('passwords'),[vault,setVault]=useState<VaultStatus|null>(null),[busy,setBusy]=useState(false),[error,setError]=useState(''),[notice,setNotice]=useState('');
 const [master,setMaster]=useState(''),[masterAgain,setMasterAgain]=useState('');
 const [credentials,setCredentials]=useState<CredentialSummary[]>([]),[account,setAccount]=useState<AccountStatus|null>(null),[sync,setSync]=useState<SyncStatus|null>(null);
 const [selection,setSelection]=useState<SyncSelection>(emptySelection),[dirty,setDirty]=useState(false),[report,setReport]=useState<SyncReport|null>(null);
 const [manual,setManual]=useState(false),[origin,setOrigin]=useState(''),[username,setUsername]=useState(''),[password,setPassword]=useState(''),[title,setTitle]=useState(''),[superId,setSuperId]=useState('');
 const [draft,setDraft]=useState<CredentialDraft|null>(null),[reveal,setReveal]=useState<{id:string;value:string}|null>(null);
 const [mode,setMode]=useState<'login'|'register'|'recover'>('login'),[server,setServer]=useState(DEFAULT_ACCOUNT_SERVER),[accountName,setAccountName]=useState(''),[accountPass,setAccountPass]=useState(''),[accountAgain,setAccountAgain]=useState(''),[code,setCode]=useState('');
 const [authStep,setAuthStep]=useState('');
 const [recovery,setRecovery]=useState(''),[recoveryKey,setRecoveryKey]=useState(''),[backupCodes,setBackupCodes]=useState<string[]>([]),[enrollment,setEnrollment]=useState<Enrollment|null>(null);
 const [changePass,setChangePass]=useState(false),[oldPass,setOldPass]=useState(''),[newPass,setNewPass]=useState(''),[newAgain,setNewAgain]=useState('');
 const [removeAccount,setRemoveAccount]=useState(false),[deleteConfirm,setDeleteConfirm]=useState('');
 const alive=useRef(true),revision=useRef(0),workRunning=useRef(false),panel=useRef<HTMLElement>(null);
 async function refresh(){
  const token=++revision.current;const next=await security.vault('status');if(!alive.current||token!==revision.current)return;setVault(next);
  if(!next.unlocked){setCredentials([]);setAccount(null);setSync(null);setReveal(null);setEnrollment(null);setRecoveryKey('');setBackupCodes([]);return;}
  const rows=await security.passwords<CredentialSummary[]>('list');if(!alive.current||token!==revision.current)return;setCredentials(rows);
  const state=await security.account('status');if(!alive.current||token!==revision.current)return;setAccount(state);setServer(state.server||DEFAULT_ACCOUNT_SERVER);
  const status=await security.sync('status');if(!alive.current||token!==revision.current)return;setSync(status);setSelection(status.selection||emptySelection());setDirty(false);setReport(status.report||null);return status;
 }
 useEffect(()=>{alive.current=true;void refresh().catch(e=>{if(alive.current)setError(errorText(e));});return()=>{alive.current=false;revision.current++;};},[]);
 useEffect(()=>{if(!vault?.unlocked)return;const timer=setInterval(()=>{void security.vault('status').then(v=>{if(alive.current&&!v.unlocked){revision.current++;setVault(v);setCredentials([]);setAccount(null);setSync(null);setReveal(null);setEnrollment(null);setRecoveryKey('');setBackupCodes([]);setNotice('Хранилище заблокировано. Разблокируйте его мастер-паролем.');}}).catch(()=>{});},15000);return()=>clearInterval(timer);},[vault?.unlocked]);
 useEffect(()=>{if(!reveal)return;const timer=setTimeout(()=>setReveal(null),30000);return()=>clearTimeout(timer);},[reveal]);
 useEffect(()=>{if(vault&&!vault.unlocked){setPassword('');setAccountPass('');setAccountAgain('');setOldPass('');setNewPass('');setNewAgain('');setRecovery('');setCode('');setDraft(null);setManual(false);}},[vault?.unlocked]);
 useEffect(()=>{setReveal(null);setPassword('');setAccountPass('');setAccountAgain('');setRecovery('');setOldPass('');setNewPass('');setNewAgain('');setCode('');},[tab,mode]);
 async function run(work:()=>Promise<unknown>){if(workRunning.current)return;workRunning.current=true;setBusy(true);setError('');setNotice('');try{await work();}catch(e){if(alive.current)setError(errorText(e));}finally{workRunning.current=false;if(alive.current)setBusy(false);}}
 async function clipboard(text:string){await invoke('bufer_zapisat',{text});setNotice('Скопировано в буфер обмена.');}
 async function unlock(e:FormEvent){e.preventDefault();const secret=master;setMaster('');setMasterAgain('');await run(async()=>{await security.vault(vault?.exists?'unlock':'create',secret);await refresh();});}
 async function lock(){setMaster('');setMasterAgain('');setPassword('');setAccountPass('');setAccountAgain('');setOldPass('');setNewPass('');setNewAgain('');setRecovery('');setCode('');setReveal(null);setDraft(null);setEnrollment(null);setRecoveryKey('');setBackupCodes([]);await run(async()=>{await security.vault('lock');await refresh();});}
 async function capture(){await run(async()=>{const target=await currentCredentialTarget();const candidate=await security.passwords<CredentialDraft>('capture',target);setDraft(candidate);setUsername(candidate.username);setTitle('');setManual(false);setReveal(null);});}
 async function savePassword(e:FormEvent){e.preventDefault();const secret=password;setPassword('');await run(async()=>{
  if(draft)await security.passwords('save_draft',{draft_id:draft.draft_id,username,title});else await security.passwords('save',{origin,username,password:secret,title,super_id:superId});
  setDraft(null);setManual(false);setOrigin('');setUsername('');setTitle('');await refresh();setNotice('Логин и пароль сохранены в зашифрованном хранилище.');
 });}
 async function authenticate(e:FormEvent){e.preventDefault();const secret=accountPass,key=recovery;setAccountPass('');setAccountAgain('');setRecovery('');await run(async()=>{
  try{
   if(!account?.configured){setAuthStep('Подключаю аккаунт NexusBro…');const standard=await security.account<{server:string}>('default_server');const selected=server.trim();await security.account('configure',{server:selected&&selected!==DEFAULT_ACCOUNT_SERVER?selected:standard.server});}
   setAuthStep(mode==='register'?'Создаю аккаунт…':mode==='recover'?'Восстанавливаю доступ…':'Выполняю вход…');
   const data=mode==='recover'?{username:accountName,recovery_key:key,new_password:secret}:{username:accountName,password:secret,...(code?{totp:code}:{})};
   const result=await security.account<AccountResult>(mode,data);setCode('');await refresh();if(result.recovery_key)setRecoveryKey(result.recovery_key);if(result.recovery_codes)setBackupCodes(result.recovery_codes);
   if(!result.logged_in){if(mode==='register'&&result.recovery_key){setNotice('Аккаунт создан. Сохраните ключ восстановления и выполните вход.');if(result.warning)setError(errorText(result.warning));setMode('login');return;}throw new Error(result.warning||'Вход не завершён. Повторите попытку.');}
   setNotice(mode==='register'?'Аккаунт NexusBro создан. Сохраните ключ восстановления.':mode==='recover'?'Доступ к аккаунту NexusBro восстановлен.':'Вход в аккаунт NexusBro выполнен.');
  }finally{setAuthStep('');}
 });}
 async function saveSelection(next=selection){await security.sync('configure',{selection:next});setDirty(false);setSync(v=>v?{...v,selection:next}:v);}
 function editSelection(next:SyncSelection){setSelection(next);setDirty(true);}
 function conflictLabel(id:string,index:number){
  const parts=id.split('/'),kind=(parts[1]||'').toLowerCase(),record=parts.slice(2).join('/');
  if(['password','passwords','credential','credentials'].includes(kind)){
   const row=credentials.find(v=>v.id===record||id.endsWith('/'+v.id));return row?'Пароль — '+(row.title||row.origin):'Пароль · запись '+(index+1);
  }
  if(['super','supers','supertab','super-tab'].includes(kind)){
   const row=sync?.available_supers.find(v=>v.id===record||id.endsWith('/'+v.id));return row?'Супер-вкладка — '+row.name:'Супер-вкладка · запись '+(index+1);
  }
  if(['settings','setting','preferences'].includes(kind))return 'Настройки интерфейса';
  if(['bookmark','bookmarks'].includes(kind))return 'Закладки';
  return 'Запись синхронизации '+(index+1);
 }
 async function resolveConflict(id:string,choice:'local'|'remote',label:string){
  if(choice==='remote'&&!window.confirm('Заменить локальную запись «'+label+'» версией из облака? Локальная версия будет заменена. Остальные записи останутся.'))return;
  await run(async()=>{
   const resolved=await security.sync<SyncStatus>('resolve',{id,choice});setSync(resolved);setReport(resolved.report||null);
   const next=await refresh()||resolved;
   if(next.report?.conflicts.length)setNotice('Версия выбрана. Остальные конфликты требуют решения.');
   else if(next.report?.errors?.length)setNotice('Версия выбрана. Проверьте оставшиеся ошибки синхронизации.');
   else setNotice('Версия выбрана. Теперь можно повторить синхронизацию.');
  });
 }
 function chooseTab(next:Tab){if(busy)return;setError('');setNotice('');setTab(next);}
 function close(){if(!busy){setReveal(null);setPassword('');setAccountPass('');setMaster('');setRecovery('');setRecoveryKey('');setBackupCodes([]);setEnrollment(null);onClose();}}
 const canSync=!!account?.logged_in&&!!account.configured;
 const supers=sync?.available_supers||[];
 let content:ReactNode;
 if(!vault)content=<p aria-live="polite">{error?'Хранилище недоступно. Повторите загрузку.':'Проверяю хранилище…'}</p>;
 else if(!vault.unlocked)content=<form className="security-unlock" onSubmit={unlock}>
  <h3>{vault.exists?'Разблокировать хранилище':'Создать защищённое хранилище'}</h3>
  <p>{tab==='account'?'Сначала защитите данные на этом устройстве мастер-паролем. Затем здесь появятся регистрация и вход в аккаунт NexusBro.':tab==='sync'?'Для синхронизации сначала разблокируйте данные на этом устройстве. Затем войдите в аккаунт NexusBro и выберите нужные категории.':'Один раз создайте хранилище и придумайте мастер-пароль. Он защищает сохранённые логины и пароли на этом устройстве.'}</p>
  <p className="security-hint">Аккаунт NexusBro для сохранения паролей на этом устройстве не нужен. Мастер-пароль не отправляется на сервер и отличается от пароля аккаунта. После создания хранилища он потребуется при запуске браузера. Ctrl+Shift+L блокирует браузер.</p>
  <ol className="security-password-steps" aria-label="Как сохраняются пароли"><li>Создайте или разблокируйте хранилище.</li><li>Войдите на сайт в обычной вкладке.</li><li>Нажмите «Сохранить» в предложении браузера — логин и пароль попадут в хранилище.</li></ol>
  <Secret label="Мастер-пароль" value={master} onChange={setMaster} minLength={vault.exists?undefined:12} autoComplete={vault.exists?'current-password':'new-password'}/>
  {!vault.exists&&<><Secret label="Повторите мастер-пароль" value={masterAgain} onChange={setMasterAgain} minLength={12} autoComplete="new-password"/><small>Не менее 12 символов. Сохраните пароль: без него локальное хранилище не открыть.</small></>}
  <button className="security-primary" disabled={busy||!master||(!vault.exists&&(master.length<12||master!==masterAgain))}>{vault.exists?'Разблокировать':'Создать хранилище'}</button>
 </form>;
 else if(tab==='passwords')content=<>
  <div className="security-section-heading"><div><h3>Сохранённые логины и пароли</h3><p>Войдите на сайт в обычной вкладке. Браузер предложит сохранить логин и пароль — запись появится здесь после нажатия «Сохранить».</p></div></div>
  <p className="security-hint">Хранилище разблокировано: предложения сохранения включены. Если предложения не было, откройте заполненную форму входа и нажмите «Сохранить с открытой страницы». При блокировке хранилища и в приватных вкладках пароли не сохраняются. Заполнение — по вашему выбору на исходном сайте.</p>
  <div className="security-actions"><button disabled={busy} onClick={()=>{setManual(v=>!v);setDraft(null);setPassword('');}}>Добавить вручную</button><button disabled={busy} onClick={()=>void capture()}>Сохранить с открытой страницы</button></div>
  {(manual||draft)&&<form className="security-card security-password-form" onSubmit={savePassword}>
   <h4>{draft?'Сохранить данные страницы':'Новый логин и пароль'}</h4>
   {draft?<p className="security-origin">{draft.origin}</p>:<label>Адрес сайта<input aria-label="Адрес сайта для пароля" type="url" placeholder="https://example.com" value={origin} onChange={e=>setOrigin(e.target.value)} required/></label>}
   <label>Логин<input aria-label="Логин сайта" autoComplete="off" value={username} onChange={e=>setUsername(e.target.value)} required/></label>
   {!draft&&<Secret label="Пароль сайта" value={password} onChange={setPassword}/>}
   <label>Название<input aria-label="Название записи пароля" value={title} onChange={e=>setTitle(e.target.value)} placeholder="Необязательно"/></label>
   {!draft&&<label>Супер-вкладка<Vybor aria-label="Супер-вкладка пароля" value={superId} onChange={e=>setSuperId(e.target.value)} disabled={busy}><option value="">Выберите супер-вкладку</option>{supers.map(v=><option key={v.id} value={v.id}>{v.name}</option>)}</Vybor></label>}
   {draft&&<small>Пароль страницы остаётся в памяти приложения до вашего подтверждения. Неотправленная запись истекает автоматически.</small>}
   <div className="security-actions"><button type="button" disabled={busy} onClick={()=>{setManual(false);setDraft(null);setPassword('');}}>Отмена добавления</button><button className="security-primary" disabled={busy||(!draft&&(!password||!superId))}>Сохранить пароль</button></div>
  </form>}
  {!credentials.length?<p className="security-empty">Сохранённых паролей пока нет.</p>:<ul className="security-password-list">{credentials.map(row=><li key={row.id} className="security-card"><div className="security-credential"><strong>{row.title||row.origin}</strong><span>{row.origin}</span><span>{row.username}</span></div><div className="security-actions"><button disabled={busy} onClick={()=>void run(async()=>{await security.passwords('fill',{id:row.id,...await currentCredentialTarget()});setNotice('Данные заполнены на текущей странице.');})}>Заполнить на странице</button><button disabled={busy} aria-label={'Показать пароль: '+(row.title||row.origin)} onClick={()=>{if(reveal?.id===row.id){setReveal(null);return;}void run(async()=>setReveal({id:row.id,value:await security.passwords<string>('reveal',{id:row.id})}));}}>{reveal?.id===row.id?'Скрыть пароль':'Показать пароль'}</button><button className="security-danger" disabled={busy} onClick={()=>{if(window.confirm('Удалить сохранённый пароль для '+row.origin+'?'))void run(async()=>{await security.passwords('delete',{id:row.id});setReveal(null);await refresh();setNotice('Запись удалена.');});}}>Удалить</button></div>{reveal?.id===row.id&&<div className="security-revealed"><input aria-label={'Пароль: '+(row.title||row.origin)} type="text" readOnly autoComplete="off" value={reveal.value}/><small>Скроется через 30 секунд.</small></div>}</li>)}</ul>}
 </>;
 else if(tab==='account')content=<>
  <h3>Отдельный аккаунт NexusBro</h3><p>Создайте аккаунт или войдите, чтобы синхронизировать выбранные данные между устройствами. Подключение NexusBro настроится автоматически. Пароль аккаунта отличается от мастер-пароля этого устройства.</p>
  {account?.configured&&account.server!==DEFAULT_ACCOUNT_SERVER&&<p className="security-hint">Используется выбранный вами сервер: {account.server}</p>}
  <details className="security-advanced-server"><summary>Другой сервер синхронизации</summary><p>Необязательно. Меняйте только для подключения к собственному сервису NexusBro. Для обычного входа ничего настраивать не нужно.</p><form className="security-server" onSubmit={e=>{e.preventDefault();void run(async()=>{await security.account('configure',{server:server.trim()});await refresh();setNotice('Сервер подключён. Теперь войдите или создайте аккаунт.');});}}><label>Сервер синхронизации NexusBro<input aria-label="Сервер синхронизации NexusBro" type="url" value={server} onChange={e=>setServer(e.target.value)} required disabled={busy||!!account?.logged_in}/></label><button disabled={busy||!!account?.logged_in||!server.trim()}>Сохранить сервер</button></form></details>
  {!account?.logged_in?<>
   <div className="security-segment" role="group" aria-label="Действие с аккаунтом">{([['login','Войти'],['register','Зарегистрироваться'],['recover','Восстановить доступ']] as const).map(([m,label])=><button key={m} className={mode===m?'selected':''} disabled={busy} onClick={()=>{setMode(m);setError('');setNotice('');}}>{label}</button>)}</div>
   <form className="security-account-form" onSubmit={authenticate}>
    <label>Имя аккаунта<input aria-label="Имя аккаунта NexusBro" value={accountName} onChange={e=>setAccountName(e.target.value)} autoComplete="username" required/></label>
    {mode==='recover'&&<Secret label="Ключ восстановления аккаунта" value={recovery} onChange={setRecovery}/>}
    <Secret label={mode==='recover'?'Новый пароль аккаунта':'Пароль аккаунта NexusBro'} value={accountPass} onChange={setAccountPass} minLength={mode==='login'?undefined:12} autoComplete={mode==='login'?'current-password':'new-password'}/>
    {mode!=='login'&&<Secret label="Повторите пароль аккаунта" value={accountAgain} onChange={setAccountAgain} minLength={12} autoComplete="new-password"/>}
    {mode==='login'&&<label>Код двухфакторной аутентификации<input aria-label="Код 2FA для входа" value={code} onChange={e=>setCode(e.target.value)} autoComplete="one-time-code" placeholder="Если включена 2FA"/></label>}
    <button className="security-primary" disabled={busy||!accountName||!accountPass||(mode!=='login'&&(accountPass.length<12||accountPass!==accountAgain))||(mode==='recover'&&!recovery)}>{authStep||(mode==='login'?'Войти в NexusBro':mode==='register'?'Создать аккаунт NexusBro':'Восстановить аккаунт')}</button>
    {authStep&&<p className="security-connecting" aria-live="polite">{authStep}</p>}
   </form>
  </>:<>
   <div className="security-card security-account-status"><strong>Выполнен вход: {account.username}</strong><span>Двухфакторная аутентификация: {account.totp_enabled?'включена':'не включена'}</span><button disabled={busy} onClick={()=>void run(async()=>{await security.account('logout');setMode('login');setEnrollment(null);setRecoveryKey('');setBackupCodes([]);await refresh();setNotice('Вы вышли из аккаунта. Локальные пароли сохранены.');})}>Выйти из аккаунта</button></div>
   {!account.totp_enabled&&!enrollment&&<button disabled={busy} onClick={()=>void run(async()=>{setEnrollment(await security.account<Enrollment>('totp_enroll'));setCode('');})}>Настроить двухфакторную аутентификацию</button>}
   {enrollment&&<form className="security-card" onSubmit={e=>{e.preventDefault();const next=code;setCode('');void run(async()=>{const result=await security.account<AccountResult>('totp_verify',{code:next});setEnrollment(null);setBackupCodes(result.recovery_codes||[]);await refresh();setNotice('Двухфакторная аутентификация включена.');});}}><h4>Подтвердите настройку 2FA</h4><p>Добавьте этот ключ вручную в приложение-аутентификатор и введите полученный код. До подтверждения 2FA не включена.</p><code className="security-emergency">{enrollment.secret}</code><button type="button" disabled={busy} onClick={()=>void run(()=>clipboard(enrollment.otpauth_uri))}>Скопировать ссылку настройки 2FA</button><label>Код из приложения<input aria-label="Код подтверждения 2FA" value={code} onChange={e=>setCode(e.target.value)} autoComplete="one-time-code" required/></label><div className="security-actions"><button type="button" disabled={busy} onClick={()=>{setEnrollment(null);setCode('');}}>Отмена настройки 2FA</button><button disabled={busy||!code} className="security-primary">Подтвердить и включить 2FA</button></div></form>}
   {account.totp_enabled&&<form className="security-card" onSubmit={e=>{e.preventDefault();if(!window.confirm('Отключить двухфакторную аутентификацию аккаунта NexusBro?'))return;const next=code;setCode('');void run(async()=>{await security.account('totp_disable',{code:next});await refresh();setNotice('Двухфакторная аутентификация отключена.');});}}><label>Текущий код или резервный код 2FA<input aria-label="Код для отключения 2FA" value={code} onChange={e=>setCode(e.target.value)} autoComplete="one-time-code" required/></label><button disabled={busy||!code}>Отключить 2FA</button></form>}
   <button disabled={busy} onClick={()=>{setChangePass(v=>!v);setOldPass('');setNewPass('');setNewAgain('');}}>Изменить пароль аккаунта</button>
   {changePass&&<form className="security-card" onSubmit={e=>{e.preventDefault();const old=oldPass,next=newPass;setOldPass('');setNewPass('');setNewAgain('');void run(async()=>{await security.account('change_password',{old_password:old,new_password:next,...(code?{totp:code}:{})});setChangePass(false);setCode('');await refresh();setNotice('Пароль аккаунта изменён. Мастер-пароль устройства остался прежним.');});}}><Secret label="Текущий пароль аккаунта" value={oldPass} onChange={setOldPass}/><Secret label="Новый пароль аккаунта" value={newPass} onChange={setNewPass} minLength={12} autoComplete="new-password"/><Secret label="Повторите новый пароль аккаунта" value={newAgain} onChange={setNewAgain} minLength={12} autoComplete="new-password"/>{account.totp_enabled&&<label>Код 2FA для смены пароля<input aria-label="Код 2FA для смены пароля" value={code} onChange={e=>setCode(e.target.value)} autoComplete="one-time-code" required/></label>}<button disabled={busy||!oldPass||newPass.length<12||newPass!==newAgain||(account.totp_enabled&&!code)} className="security-primary">Сохранить новый пароль аккаунта</button></form>}
   <details className="security-danger-zone" open={removeAccount} onToggle={e=>setRemoveAccount(e.currentTarget.open)}><summary>Удаление аккаунта</summary><p>Облачные данные аккаунта NexusBro будут удалены. Локальное хранилище паролей останется.</p><form onSubmit={e=>{e.preventDefault();const secret=accountPass;setAccountPass('');void run(async()=>{await security.account('delete_account',{password:secret,...(code?{totp:code}:{})});setDeleteConfirm('');setCode('');setRemoveAccount(false);setMode('login');await refresh();setNotice('Аккаунт NexusBro удалён. Локальные записи сохранены.');});}}><label>Введите имя аккаунта для подтверждения<input aria-label="Подтверждение удаления аккаунта" value={deleteConfirm} onChange={e=>setDeleteConfirm(e.target.value)} autoComplete="off"/></label><Secret label="Пароль для удаления аккаунта" value={accountPass} onChange={setAccountPass}/>{account.totp_enabled&&<label>Код 2FA<input aria-label="Код 2FA для удаления аккаунта" value={code} onChange={e=>setCode(e.target.value)} autoComplete="one-time-code"/></label>}<button className="security-danger" disabled={busy||deleteConfirm!==account.username||!accountPass||(account.totp_enabled&&!code)}>Удалить аккаунт безвозвратно</button></form></details>
  </>}
 </>;
 else content=<>
  <h3>Что синхронизировать</h3><p>Все категории выключены по умолчанию. Только выбранные данные шифруются на устройстве и передаются в отдельный аккаунт NexusBro.</p>
  {!canSync&&<p className="security-hint">Для синхронизации войдите в аккаунт NexusBro. Выбор данных можно подготовить заранее.</p>}
  <div className="security-card security-sync-options">
   <label><input aria-label="Настройки интерфейса" type="checkbox" checked={selection.settings} disabled={busy} onChange={e=>editSelection({...selection,settings:e.target.checked})}/><span>Настройки интерфейса<small>Оформление и переносимые настройки, без путей на диске и данных подключения</small></span></label>
   <label><input type="checkbox" checked={selection.bookmarks} disabled={busy} onChange={e=>editSelection({...selection,bookmarks:e.target.checked})}/><span>Закладки</span></label>
  </div>
  <fieldset className="security-sync-items" disabled={busy}><legend>Супер-вкладки</legend><label className="security-sync-all"><input aria-label="Все супер-вкладки, включая новые" type="checkbox" checked={selection.supers.includes('*')} onChange={e=>editSelection({...selection,supers:e.target.checked?['*']:[]})}/><span>Все супер-вкладки, включая новые<small>В том числе созданные на других устройствах. Приватные супер-вкладки исключены.</small></span></label>{supers.length?supers.map(v=><label key={v.id}><input type="checkbox" checked={selection.supers.includes('*')||selection.supers.includes(v.id)} disabled={selection.supers.includes('*')} onChange={()=>editSelection({...selection,supers:toggleId(selection.supers,v.id)})}/><span>{v.name}</span></label>):<p className="security-empty">Нет доступных обычных супер-вкладок.</p>}<small>Только выбранные определения. Приватные вкладки, открытые страницы, cookies и параметры прокси не передаются.</small></fieldset>
  <fieldset className="security-sync-items" disabled={busy}><legend>Пароли</legend><label className="security-sync-all"><input aria-label="Все пароли, включая новые" type="checkbox" checked={selection.passwords.includes('*')} onChange={e=>editSelection({...selection,passwords:e.target.checked?['*']:[]})}/><span>Все пароли, включая новые<small>В том числе сохранённые на других устройствах.</small></span></label>{credentials.length?credentials.map(v=><label key={v.id}><input type="checkbox" checked={selection.passwords.includes('*')||selection.passwords.includes(v.id)} disabled={selection.passwords.includes('*')} onChange={()=>editSelection({...selection,passwords:toggleId(selection.passwords,v.id)})}/><span>{v.title||v.origin}<small>{v.username}</small></span></label>):<p className="security-empty">Сначала сохраните пароль.</p>}<small>Отключение выбора прекращает синхронизацию и не удаляет данные на других устройствах.</small></fieldset>
  <p className="security-hint">История, загрузки, cookies и данные приватных вкладок не синхронизируются.</p>
  <div className="security-actions"><button disabled={busy||!dirty} onClick={()=>void run(async()=>{await saveSelection();setNotice('Выбор данных сохранён. Синхронизация ещё не запускалась.');})}>Сохранить выбор</button><button className="security-primary" disabled={busy||!canSync} onClick={()=>void run(async()=>{if(dirty)await saveSelection();const attempt=await security.sync<SyncReport>('run');const latest=await refresh();const state=latest?.report||attempt;const next={...state,conflicts:[...new Set([...attempt.conflicts,...state.conflicts])],errors:[...new Set([...(attempt.errors||[]),...(state.errors||[])])]};setReport(next);if(!next.conflicts.length&&!next.errors.length)setNotice('Синхронизация завершена.');})}>Синхронизировать сейчас</button></div>
  <p>Последняя синхронизация: {dateText(sync?.last_sync)}</p>
  {report&&<div className="security-card security-sync-report"><strong>Отправлено: {report.pushed} · Получено: {report.pulled}</strong>{report.conflicts.length>0&&<><p>Обнаружены конфликты. Локальные изменения сохранены; данные не перезаписаны автоматически.</p>{dirty&&<p className="security-hint">Сохраните выбор данных перед разрешением конфликтов.</p>}<ul className="security-conflicts">{report.conflicts.map((id,i)=>{const label=conflictLabel(id,i);return <li key={id} role="group" aria-label={"Конфликт: "+label}><strong>{label}</strong><div className="security-actions"><button disabled={busy||!canSync||dirty} onClick={()=>void resolveConflict(id,'local',label)}>Оставить на этом устройстве</button><button disabled={busy||!canSync||dirty} onClick={()=>void resolveConflict(id,'remote',label)}>Взять из облака</button></div></li>;})}</ul></>}{!!report.errors?.length&&<ul role="alert">{report.errors.map((v,i)=><li key={i}>{v}</li>)}</ul>}</div>}
 </>;
 return <div className="zatemnenie security-backdrop" onClick={close}><section ref={panel} className="dialog security-panel" role="dialog" aria-label="Пароли, аккаунт и синхронизация NexusBro" aria-modal="true" aria-busy={busy} onClick={e=>e.stopPropagation()} onKeyDown={e=>{if(e.key==='Escape'){e.preventDefault();e.stopPropagation();if(reveal){setReveal(null);return;}close();}}}>
  <header className="security-header"><h2><Icon name="shield" size={23}/>Пароли и синхронизация</h2><button type="button" disabled={busy} aria-label="Закрыть пароли и синхронизацию" onClick={close}><Icon name="close"/></button></header>
  <nav className="security-tabs" aria-label="Разделы безопасности">{([['passwords','Пароли'],['account','Аккаунт'],['sync','Синхронизация']] as const).map(([key,label])=><button type="button" key={key} aria-pressed={tab===key} className={tab===key?'selected':''} disabled={busy} onClick={()=>chooseTab(key)}>{label}</button>)}{vault?.unlocked&&<button type="button" className="security-lock" disabled={busy} onClick={()=>void lock()}>Заблокировать хранилище</button>}</nav>
  <div className="security-content"><fieldset className="security-work" disabled={busy}>{content}</fieldset>
   {recoveryKey&&<div className="security-card security-recovery" role="region" aria-label="Ключ восстановления аккаунта"><h3>Сохраните ключ восстановления</h3><p>Он показывается один раз. Храните отдельно от устройства и пароля аккаунта.</p><code className="security-emergency">{recoveryKey}</code><div className="security-actions"><button disabled={busy} onClick={()=>void run(()=>clipboard(recoveryKey))}>Скопировать ключ восстановления</button><button disabled={busy} onClick={()=>setRecoveryKey('')}>Ключ сохранён</button></div></div>}
   {!!backupCodes.length&&<div className="security-card security-recovery" role="region" aria-label="Резервные коды 2FA"><h3>Сохраните резервные коды 2FA</h3><p>Каждый код используется один раз. После закрытия этого окна коды больше не показываются.</p><ul className="security-backup-codes">{backupCodes.map((v,i)=><li key={i}><code>{v}</code></li>)}</ul><div className="security-actions"><button disabled={busy} onClick={()=>void run(()=>clipboard(backupCodes.join('\n')))}>Скопировать резервные коды</button><button disabled={busy} onClick={()=>setBackupCodes([])}>Резервные коды сохранены</button></div></div>}
   {error&&<p role="alert" className="security-error">{error}</p>}{notice&&<p role="status" className="security-notice">{notice}</p>}
   {!vault&&error&&<button disabled={busy} onClick={()=>void run(refresh)}>Повторить загрузку</button>}
  </div>
  <footer className="security-footer"><small>{busy?'Выполняется…':vault?.unlocked?'Хранилище разблокировано на этом устройстве':'Хранилище заблокировано'}</small><button disabled={busy} onClick={close}>Закрыть</button></footer>
 </section></div>;
}
