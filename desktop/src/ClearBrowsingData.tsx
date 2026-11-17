import {useState} from 'react';
import {invoke} from '@tauri-apps/api/core';

type Receipt={istoriya:number;zagruzki:number};
export default function ClearBrowsingData({superId,onClose}:{superId:string|null;onClose:()=>void}){
 const [period,setPeriod]=useState('hour'),[history,setHistory]=useState(true),[downloads,setDownloads]=useState(false);
 const [cookies,setCookies]=useState(false),[cache,setCache]=useState(false),[busy,setBusy]=useState(false);
 const [error,setError]=useState(''),[result,setResult]=useState('');
 async function clear(){
  if(busy||(!history&&!downloads&&!cookies&&!cache))return;
  setBusy(true);setError('');setResult('');let local:Receipt|null=null;
  try{
   local=await invoke<Receipt>('dannye_ochistit',{period,istoriya:history,zagruzki:downloads});
   if((cookies||cache)&&superId){
    const ack=await invoke<{cookies:boolean;cache:boolean}>('ochistit_dannye_saytov',{id:superId,cookies,cache});
    if((cookies&&!ack.cookies)||(cache&&!ack.cache))throw new Error('Движок не подтвердил полную очистку выбранных данных сайтов');
   }
   setResult(`Очистка завершена. Удалено записей истории: ${local.istoriya}; записей загрузок: ${local.zagruzki}.${cookies?' Cookies очищены.':''}${cache?' Кэш очищен.':''}`);
  }catch(e){
   setError(`${local?`История: удалено ${local.istoriya}; список загрузок: удалено ${local.zagruzki}. `:'Часть операций могла выполниться. '}Не удалось завершить очистку: ${String(e)}`);
  }finally{window.dispatchEvent(new CustomEvent('nexusbro-browsing-data-cleared'));setBusy(false);}
 }
 return <div className="zatemnenie" onClick={()=>{if(!busy)onClose();}}><section className="dialog clear-data-panel" role="dialog" aria-label="Удаление данных браузера" aria-modal="true" onClick={e=>e.stopPropagation()} onKeyDown={e=>{if(e.key==='Escape'&&!busy){e.stopPropagation();onClose();}}}>
  <header><h2>Удаление данных браузера</h2><button aria-label="Закрыть очистку данных" disabled={busy} onClick={onClose}>×</button></header>
  <label>Период истории <select aria-label="Период очистки истории" value={period} disabled={busy||!history} onChange={e=>setPeriod(e.target.value)}>
   <option value="hour">Последний час</option><option value="day">Последние 24 часа</option><option value="week">Последние 7 дней</option><option value="month">Последние 4 недели</option><option value="all">За всё время</option>
  </select></label>
  <div className="clear-data-options">
   <label><input type="checkbox" checked={history} disabled={busy} onChange={e=>setHistory(e.target.checked)}/>История посещений — все супер-вкладки, выбранный период</label>
   <label><input type="checkbox" checked={downloads} disabled={busy} onChange={e=>setDownloads(e.target.checked)}/>Список завершённых загрузок — за всё время</label>
   <label><input type="checkbox" checked={cookies} disabled={busy||!superId} onChange={e=>setCookies(e.target.checked)}/>Cookies текущей супер-вкладки — за всё время</label>
   <label><input type="checkbox" checked={cache} disabled={busy||!superId} onChange={e=>setCache(e.target.checked)}/>Кэш текущей супер-вкладки — за всё время</label>
  </div>
  <p>Закладки и скачанные файлы останутся. Активные загрузки продолжаются. Cookies и кэш очищаются только для текущей супер-вкладки, включая открытые в ней окна.</p>
  {cookies&&<p>После удаления cookies потребуется снова войти на некоторые сайты. Локальное хранилище сайтов (localStorage) этой командой не удаляется.</p>}
  {error&&<p role="alert">{error}</p>}{result&&<p role="status">{result}</p>}
  <footer><button disabled={busy} onClick={onClose}>{result?'Готово':'Отмена'}</button><button disabled={busy||(!history&&!downloads&&!cookies&&!cache)} onClick={()=>void clear()}>{busy?'Удаление…':'Удалить данные'}</button></footer>
 </section></div>;
}
