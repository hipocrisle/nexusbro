import {useEffect,useState,type ReactNode} from 'react';
import {createPortal} from 'react-dom';
import {invoke} from '@tauri-apps/api/core';
export type MenuItem={label:string;shortcut?:string;disabled?:boolean;keepsHidden?:boolean;action:()=>void|Promise<unknown>}|null;
export default function BrowserMenu({label,children,items,className=''}:{label:string;children:ReactNode;items:MenuItem[];className?:string}){
 const [error,setError]=useState('');
 const [at,setAt]=useState<{x:number;y:number}|null>(null);
 async function close(){setAt(null);await invoke('pokazat_dvizhki',{vidno:true});}
 useEffect(()=>{if(!at)return;const key=(e:KeyboardEvent)=>{if(e.key==='Escape'){e.preventDefault();void close();}};document.addEventListener('keydown',key);return()=>document.removeEventListener('keydown',key);},[at]);
 return <><button type="button" className={'browser-menu-button '+className} title={label} aria-label={label} aria-haspopup="menu" aria-expanded={!!at} onClick={async e=>{const r=e.currentTarget.getBoundingClientRect();await invoke('pokazat_dvizhki',{vidno:false});setError('');setAt({x:Math.max(8,Math.min(r.left,innerWidth-372)),y:Math.min(r.bottom+4,Math.max(8,innerHeight-items.length*35-20))});}}>{children}</button>{error&&<span role="alert" className="oshibka-navigacii" title={error}>{error}</span>}{at&&createPortal(<div className="zatemnenie-prozrachnoe" onMouseDown={()=>void close()} onContextMenu={e=>{e.preventDefault();void close();}}><div role="menu" aria-label={label} className="vypadajka browser-menu" style={{left:at.x,top:at.y}} onMouseDown={e=>e.stopPropagation()}>{items.map((item,i)=>item?<button type="button" role="menuitem" disabled={item.disabled} key={i} onClick={async()=>{setAt(null);try{if(!item.keepsHidden)await invoke('pokazat_dvizhki',{vidno:true});await item.action();}catch(e){setError(String(e));await invoke('pokazat_dvizhki',{vidno:true});}}}><span>{item.label}</span>{item.shortcut&&<kbd>{item.shortcut}</kbd>}</button>:<hr key={i}/>)}</div></div>,document.body)}</>;
}
