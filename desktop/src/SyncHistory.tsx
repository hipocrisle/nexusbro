import {useState} from 'react';
import {security} from './security';
type Row={entry_id:string;name:string;reason:string;created_at:number;currently_exists:boolean};
type Preview={entry_id:string;name:string;guard:string;restores_saved_only:boolean;previous:{nazvanie?:string};current:{nazvanie?:string}|null};
export default function SyncHistory({onChanged}:{onChanged:()=>Promise<unknown>}){
 const [rows,setRows]=useState<Row[]|null>(null),[preview,setPreview]=useState<Preview|null>(null),[busy,setBusy]=useState(false),[error,setError]=useState(''),[notice,setNotice]=useState('');
 async function run(work:()=>Promise<unknown>){if(busy)return;setBusy(true);setError('');setNotice('');try{await work();}catch(e){setError(String(e));}finally{setBusy(false);}}
 async function load(){const value=await security.sync<{entries:Row[]}>('history');setRows(value.entries);}
 return <details className="security-card"><summary>Корзина и история супервкладок</summary><p>Локальные зашифрованные копии настроек перед удалением или заменой из облака. История хранится на этом устройстве, до 100 записей. Cookies, пароли сайтов и удалённые вкладки этим действием не восстанавливаются.</p><button disabled={busy} onClick={()=>void run(load)}>Показать историю</button>
 {rows&&(!rows.length?<p>Сохранённых изменений пока нет.</p>:<ul className="security-password-list">{rows.map(row=><li className="security-card" key={row.entry_id}><strong>{row.name}</strong><small>{new Date(row.created_at).toLocaleString()} · {row.currently_exists?'Предыдущие настройки':'Удалённая супервкладка'}</small><button disabled={busy} onClick={()=>void run(async()=>setPreview(await security.sync<Preview>('history_preview',{entry_id:row.entry_id})))}>Посмотреть восстановление</button></li>)}</ul>)}
 {preview&&<section className="security-card" aria-label="Предпросмотр восстановления"><h3>{preview.name}</h3><p>{preview.restores_saved_only?'Супервкладка вернётся в сохранённые. Сайты не откроются автоматически, прежний сетевой путь сохранится.':'Будут возвращены предыдущие настройки. Текущий сетевой путь сохранится.'}</p><p>Автоматической отправки в облако нет. После проверки можно синхронизировать вручную.</p><button disabled={busy} onClick={()=>void run(async()=>{await security.sync('history_restore',{entry_id:preview.entry_id,guard:preview.guard,confirm:true});setPreview(null);await load();await onChanged();setNotice('Настройки супервкладки восстановлены на этом устройстве.');})}>Подтвердить восстановление</button><button disabled={busy} onClick={()=>setPreview(null)}>Отмена</button></section>}
 {error&&<p role="alert">{error}</p>}{notice&&<p role="status">{notice}</p>}
 </details>;
}
