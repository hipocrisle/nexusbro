import {useEffect,useState} from 'react';
import Vybor from './Vybor';
import {invoke} from '@tauri-apps/api/core';

type LocalSize={istoriya:number;istoriya_vsego:number;zagruzki:number;legacy?:number};
type SiteSize={cookies:number|null;cache_bytes:number|null;cache_memory:boolean};
function bytes(value:number){return new Intl.NumberFormat('ru-RU',{maximumFractionDigits:1}).format(value/(value>=1048576?1048576:1024))+(value>=1048576?' МБ':' КБ');}
type Receipt={istoriya:number;zagruzki:number};
export default function ClearBrowsingData({superId,superName,onClose}:{superId:string|null;superName:string;onClose:()=>void}){
 const [period,setPeriod]=useState('hour'),[history,setHistory]=useState(true),[downloads,setDownloads]=useState(false);
 const [cookies,setCookies]=useState(false),[cache,setCache]=useState(false),[busy,setBusy]=useState(false);
 const [legacyHistory,setLegacyHistory]=useState(false);
 const [error,setError]=useState(''),[result,setResult]=useState('');
 const [localSize,setLocalSize]=useState<LocalSize|null>(null),[siteSize,setSiteSize]=useState<SiteSize|null>(null);
 const [localFailed,setLocalFailed]=useState(false),[siteFailed,setSiteFailed]=useState(false),[revision,setRevision]=useState(0);
 useEffect(()=>{let live=true;setLocalSize(null);setLocalFailed(false);
  invoke<LocalSize>('dannye_obem',{period,superId}).then(v=>{if(live){if(v&&Number.isFinite(v.istoriya))setLocalSize(v);else setLocalFailed(true);}}).catch(()=>{if(live)setLocalFailed(true);});
  return()=>{live=false;};
 },[period,superId,revision]);
 useEffect(()=>{let live=true;setSiteSize(null);setSiteFailed(false);
  if(superId)invoke<SiteSize>('dannye_saytov_statistika',{id:superId}).then(v=>{if(live){if(v&&'cookies' in v)setSiteSize(v);else setSiteFailed(true);}}).catch(()=>{if(live)setSiteFailed(true);});
  return()=>{live=false;};
 },[superId,revision]);
 const unknown=(failed:boolean)=>failed?'Не удалось подсчитать':'Подсчёт…';

 async function clear(){
  if(busy||(!history&&!downloads&&!cookies&&!cache))return;
  setBusy(true);setError('');setResult('');let local:Receipt|null=null;
  try{
   local=await invoke<Receipt>('dannye_ochistit',{period,istoriya:history,zagruzki:downloads,superId,legacyHistory});
   if((cookies||cache)&&superId){
    const ack=await invoke<{cookies:boolean;cache:boolean}>('ochistit_dannye_saytov',{id:superId,cookies,cache});
    if((cookies&&!ack.cookies)||(cache&&!ack.cache))throw new Error('Движок не подтвердил полную очистку выбранных данных сайтов');
   }
   setResult(`Очистка завершена. Удалено записей истории: ${local.istoriya}; записей загрузок: ${local.zagruzki}.${cookies?' Cookies очищены.':''}${cache?' Кэш очищен.':''}`);
  }catch(e){
   setError(`${local?`История: удалено ${local.istoriya}; список загрузок: удалено ${local.zagruzki}. `:'Часть операций могла выполниться. '}Не удалось завершить очистку: ${String(e)}`);
  }finally{window.dispatchEvent(new CustomEvent('nexusbro-browsing-data-cleared'));setBusy(false);setRevision(v=>v+1);}
 }
 return <div className="zatemnenie" onClick={()=>{if(!busy)onClose();}}><section className="dialog clear-data-panel" role="dialog" aria-label="Удаление данных супер-вкладки" aria-modal="true" onClick={e=>e.stopPropagation()} onKeyDown={e=>{if(e.key==='Escape'&&!busy){e.stopPropagation();onClose();}}}>
  <header><h2>Удаление данных супер-вкладки «{superName}»</h2><button aria-label="Закрыть очистку данных" disabled={busy} onClick={onClose}>×</button></header>
  <label>Период истории <Vybor aria-label="Период очистки истории" value={period} disabled={busy||!history} onChange={e=>setPeriod(e.target.value)}>
   <option value="hour">Последний час</option><option value="day">Последние 24 часа</option><option value="week">Последние 7 дней</option><option value="month">Последние 4 недели</option><option value="all">За всё время</option>
  </Vybor></label>
  <div className="clear-data-options">
   <label><input aria-label="История посещений — текущая супер-вкладка, выбранный период" type="checkbox" checked={history} disabled={busy} onChange={e=>setHistory(e.target.checked)}/><span className="clear-data-option">История посещений<small data-testid="history-size">{localSize?`${localSize.istoriya} записей за выбранный период · ${localSize.istoriya_vsego} за всё время`:unknown(localFailed)}</small><small>Текущая супер-вкладка · выбранный период</small></span></label>
   <label><input aria-label="Список завершённых загрузок текущей супер-вкладки — за всё время" type="checkbox" checked={downloads} disabled={busy} onChange={e=>setDownloads(e.target.checked)}/><span className="clear-data-option">Список завершённых загрузок<small data-testid="downloads-size">{localSize?`${localSize.zagruzki} записей за всё время`:unknown(localFailed)}</small><small>Файлы на диске останутся</small></span></label>
   <label><input aria-label="Cookies текущей супер-вкладки — за всё время" type="checkbox" checked={cookies} disabled={busy||!superId} onChange={e=>setCookies(e.target.checked)}/><span className="clear-data-option">Cookies<small data-testid="cookies-size">{!superId?'Супер-вкладка не выбрана':siteSize?(siteSize.cookies===null?'Количество недоступно':`${siteSize.cookies} cookies за всё время`):unknown(siteFailed)}</small><small>Текущая супер-вкладка · за всё время</small></span></label>
   <label><input aria-label="Кэш текущей супер-вкладки — за всё время" type="checkbox" checked={cache} disabled={busy||!superId} onChange={e=>setCache(e.target.checked)}/><span className="clear-data-option">Кэш страниц<small data-testid="cache-size">{!superId?'Супер-вкладка не выбрана':siteSize?(siteSize.cache_bytes===null?'Размер недоступен':siteSize.cache_memory?`${siteSize.cache_bytes?bytes(siteSize.cache_bytes)+' на диске · ':''}Кэш в памяти — размер недоступен`:`${bytes(siteSize.cache_bytes)} на диске за всё время`):unknown(siteFailed)}</small><small>Текущая супер-вкладка · HTTP-кэш на диске, без данных сайтов</small></span></label>
  </div>
  {history&&!!localSize?.legacy&&<label><input type="checkbox" checked={legacyHistory} disabled={busy} onChange={e=>setLegacyHistory(e.target.checked)}/>Также удалить старую общую историю без привязки к Super ({localSize.legacy} записей, выбранный период)</label>}
  <p>Закладки и скачанные файлы останутся. Активные загрузки продолжаются. Удаляются только данные супер-вкладки «{superName}», включая открытые в ней окна. Данные других супер-вкладок останутся.</p>
  {cookies&&<p>После удаления cookies потребуется снова войти на некоторые сайты. Локальное хранилище сайтов (localStorage) этой командой не удаляется.</p>}
  {error&&<p role="alert">{error}</p>}{result&&<p role="status">{result}</p>}
  <footer><button disabled={busy} onClick={onClose}>{result?'Готово':'Отмена'}</button><button disabled={busy||(!history&&!downloads&&!cookies&&!cache)} onClick={()=>void clear()}>{busy?'Удаление…':'Удалить данные'}</button></footer>
 </section></div>;
}
