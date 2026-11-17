import {useEffect,useState,type RefObject} from 'react';
import {createPortal} from 'react-dom';
import {invoke} from '@tauri-apps/api/core';
type At={x:number;y:number;start:number;end:number};
export default function AddressMenu({input,onChange,onError}:{input:RefObject<HTMLInputElement>;onChange:(s:string)=>void;onError:(s:string)=>void}){
 const [at,setAt]=useState<At|null>(null);
 async function close(){setAt(null);await invoke('pokazat_dvizhki',{vidno:true});}
 useEffect(()=>{const e=input.current;if(!e)return;const menu=(event:MouseEvent)=>{event.preventDefault();event.stopPropagation();const next={x:Math.min(event.clientX,innerWidth-245),y:Math.min(event.clientY,innerHeight-270),start:e.selectionStart??0,end:e.selectionEnd??0};void invoke('pokazat_dvizhki',{vidno:false}).then(()=>setAt(next));};e.addEventListener('contextmenu',menu);return()=>e.removeEventListener('contextmenu',menu);},[]);
 useEffect(()=>{if(!at)return;const key=(e:KeyboardEvent)=>{if(e.key==='Escape')void close();};window.addEventListener('keydown',key);return()=>window.removeEventListener('keydown',key);},[at]);
 async function edit(action:string){const e=input.current;if(!e||!at)return;
  try{e.focus();e.setSelectionRange(at.start,at.end);
   if(action==='selectAll'){e.select();}
   else if(action==='paste'){const value=await navigator.clipboard.readText();e.setRangeText(value,at.start,at.end,'end');onChange(e.value);}
   else if(action==='delete'){e.setRangeText('',at.start,at.end,'end');onChange(e.value);}
   else{document.execCommand(action);onChange(e.value);}
  }catch{onError('Не удалось выполнить действие с буфером обмена. Используйте Ctrl+C / Ctrl+V.');}
  finally{await close();await invoke('fokus_obolochke');e.focus();}
 }
 if(!at)return null;
 return createPortal(<div className="zatemnenie-prozrachnoe" onMouseDown={()=>void close()}><div role="menu" aria-label="Адресная строка" className="vypadajka browser-menu address-menu" style={{left:Math.max(8,at.x),top:Math.max(8,at.y)}} onMouseDown={e=>{e.preventDefault();e.stopPropagation();}}>{([['undo','Отменить','Ctrl+Z'],['redo','Повторить','Ctrl+Shift+Z'],['cut','Вырезать','Ctrl+X'],['copy','Копировать','Ctrl+C'],['paste','Вставить','Ctrl+V'],['delete','Удалить','Del'],['selectAll','Выделить всё','Ctrl+A']] as const).map(([action,label,key])=><button type="button" role="menuitem" disabled={['cut','copy','delete'].includes(action)&&at.start===at.end} key={action} onClick={()=>void edit(action)}><span>{label}</span><kbd>{key}</kbd></button>)}</div></div>,document.body);
}
