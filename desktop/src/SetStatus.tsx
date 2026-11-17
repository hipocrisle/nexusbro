import Icon from './Icons';
import {useEffect,useState} from 'react';
import {createPortal} from 'react-dom';
import {invoke} from '@tauri-apps/api/core';
type Status={mode:string;node:string;running:boolean;starting:boolean;tunnel_running:boolean;proxy_running:boolean;reachable:boolean|null};
export default function SetStatus({id}:{id:string|null}){
 const [status,setStatus]=useState<Status|null>(null),[open,setOpen]=useState(false),[busy,setBusy]=useState(false),[error,setError]=useState('');
 const [ip,setIp]=useState<{ip:string;ms:number;checked:number}|null>(null);
 useEffect(()=>{setStatus(null);setIp(null);setError('');if(!id)return;let live=true;let timer:ReturnType<typeof setTimeout>;const refresh=async()=>{try{const s=await invoke<Status>('set_status',{id});if(live)setStatus(s);}catch{}finally{if(live)timer=setTimeout(()=>void refresh(),2000);}};void refresh();return()=>{live=false;clearTimeout(timer);};},[id]);
 async function close(){setOpen(false);await invoke('pokazat_dvizhki',{vidno:true});}
 async function check(){if(!id)return;setBusy(true);setError('');try{setIp(await invoke<{ip:string;ms:number;checked:number}>('set_proverit_ip',{id}));}catch(e){setError(String(e));}finally{setBusy(false);}}
 return <><button type="button" className={"network-button network-"+(status?.starting||status?.reachable==null?"pending":status.reachable?"up":"down")} aria-label="Сетевой выход супер-вкладки" title={(status?.node||status?.mode||'Сетевой выход')+' · '+(status?.reachable==null?'Проверка подключения':status.reachable?'Соединение доступно':'Соединение недоступно')} disabled={!id} onClick={async()=>{await invoke('pokazat_dvizhki',{vidno:false});setOpen(true);}}><Icon name="network" size={18}/><i className="network-dot"/></button>
 {open&&createPortal(<div className="zatemnenie" onClick={()=>void close()}><section className="dialog network-panel" role="dialog" aria-label="Сетевой выход" onClick={e=>e.stopPropagation()} onKeyDown={e=>{if(e.key==='Escape')void close();}}><h2>Сетевой выход</h2><dl><dt>Подключение</dt><dd>{status?.mode??'Проверяю…'}</dd>{status?.node&&<><dt>Выбранный узел</dt><dd>{status.node}</dd></>}<dt>Движок</dt><dd>{status?.starting?'Запускается':status?.running?'Работает':'Не запущен'}</dd><dt>Процесс VPN</dt><dd>{status?.tunnel_running?'Работает':'Не используется или остановлен'}</dd></dl><p className="muted">Работающий процесс не гарантирует доступность маршрута. Проверка ниже выполняется через этот выход.</p>{ip&&<p role="status">Внешний IP: <strong>{ip.ip}</strong> · {ip.ms} мс · {new Date(ip.checked*1000).toLocaleTimeString()}</p>}{error&&<p role="alert">{error}</p>}<div className="knopki"><button onClick={()=>void check()} disabled={busy}>{busy?'Проверяю выход…':'Проверить внешний IP'}</button><button onClick={()=>void close()}>Закрыть</button></div></section></div>,document.body)}
 </>;
}
