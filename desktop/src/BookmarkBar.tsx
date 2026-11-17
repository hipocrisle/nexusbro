import {useEffect,useRef,useState} from 'react';
import {invoke} from '@tauri-apps/api/core';
import {listenWindow as listen} from './windowEvents';
import BrowserMenu from './BrowserMenu';
type Bookmark={id:string;adres:string;nazvanie:string;papka:string};
export default function BookmarkBar({visible,onOpen}:{visible:boolean;onOpen:(url:string,newTab:boolean)=>Promise<void>}){
 const [items,setItems]=useState<Bookmark[]>([]),[limit,setLimit]=useState(5);const bar=useRef<HTMLDivElement>(null);
 useEffect(()=>{let live=true;const refresh=()=>invoke<Bookmark[]>('biblioteka_spisok',{razdel:'zakladki'}).then(rows=>{if(live)setItems(rows);}).catch(()=>{});void refresh();const off=listen('zakladki-izmenilis',refresh);return()=>{live=false;void off.then(f=>f());};},[]);
 useEffect(()=>{if(!visible||!bar.current)return;const observer=new ResizeObserver(()=>{if(bar.current)setLimit(Math.max(1,Math.floor((bar.current.clientWidth-42)/156)));});observer.observe(bar.current);return()=>observer.disconnect();},[visible]);
 if(!visible)return null;
 const groups=new Map<string,Bookmark[]>();for(const item of items){const key=item.papka?.split('/')[0]||'@'+item.id;groups.set(key,[...(groups.get(key)||[]),item]);}
 const entries=[...groups.entries()];
 return <nav className="bookmark-bar" aria-label="Панель закладок" ref={bar}>{!items.length&&<span>Закладки появятся здесь · Добавить страницу: Ctrl+D</span>}{entries.slice(0,limit).map(([key,rows])=>key.startsWith('@')?<button type="button" className="bookmark-bar-item" key={key} title={rows[0].nazvanie||rows[0].adres} onClick={()=>void onOpen(rows[0].adres,false)} onAuxClick={e=>{if(e.button===1){e.preventDefault();void onOpen(rows[0].adres,true);}}}><span aria-hidden="true">☆</span><span>{rows[0].nazvanie||rows[0].adres}</span></button>:<BrowserMenu key={key} label={key} className="bookmark-folder" items={rows.map(row=>({label:[...row.papka.split('/').slice(1),row.nazvanie||row.adres].join(' / '),action:()=>onOpen(row.adres,false)}))}><span aria-hidden="true">▱</span><span>{key}</span></BrowserMenu>)}{entries.length>limit&&<BrowserMenu label="Другие закладки" items={entries.slice(limit).flatMap(([key,rows])=>rows.map(row=>({label:(key.startsWith('@')?'':key+' / ')+(row.nazvanie||row.adres),action:()=>onOpen(row.adres,false)})))}>»</BrowserMenu>}</nav>;
}
