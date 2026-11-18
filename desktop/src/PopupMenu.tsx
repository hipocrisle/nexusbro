import {useEffect,useLayoutEffect,useRef,useState} from 'react';
import {invoke} from '@tauri-apps/api/core';
import './PopupMenu.css';
import {shortcutKey} from './shortcuts';
type Payload={items:({text:string;enabled:boolean}|null)[];theme:{colors:Record<string,[number,number,number]>;font_family:string;font_size:number}};
export default function PopupMenu(){
 const [menu,setMenu]=useState<Payload|null>(null),[error,setError]=useState('');
 const panel=useRef<HTMLDivElement>(null),chosen=useRef(false);
 async function dismiss(restore_focus=false){if(chosen.current)return;chosen.current=true;await invoke('popup_menu_dismiss',{restoreFocus:restore_focus}).catch(()=>{});}
 async function select(index:number){if(chosen.current)return;chosen.current=true;await invoke('popup_menu_choose',{index}).catch(()=>{});}
 useEffect(()=>{
  let alive=true;
  void invoke<Payload>('popup_menu_data').then(data=>{
   if(!alive)return;
   const root=document.documentElement;root.dataset.popup='menu';
   for(const [key,color] of Object.entries(data.theme.colors))root.style.setProperty(key,`rgb(${color.join(',')})`);
   root.style.setProperty('--font-ui',data.theme.font_family);root.style.setProperty('--font-size-ui',data.theme.font_size+'px');root.style.setProperty('--popup-row',(data.theme.font_size+20)+'px');
   root.style.colorScheme=data.theme.colors['--fon2'].reduce((a,b)=>a+b,0)>480?'light':'dark';
   setMenu(data);
  }).catch(()=>{if(alive)setError('Меню недоступно');});
  return()=>{alive=false;};
 },[]);
 useLayoutEffect(()=>{
  if(!menu)return;
  let cancelled=false;
  // Hidden WebViews may suspend requestAnimationFrame. React has committed the
  // themed DOM here; force layout before showing the owner-bound native popup.
  panel.current?.getBoundingClientRect();
  void invoke('popup_menu_ready').then(()=>{
   if(cancelled)return;
   const first=panel.current?.querySelector<HTMLButtonElement>('button:not(:disabled)');
   (first||panel.current)?.focus();
  }).catch(()=>{});
  return()=>{cancelled=true;};
 },[menu]);
 return <div ref={panel} className="nexus-popup-menu" tabIndex={-1} role="menu" aria-label="Меню NexusBro" onContextMenu={e=>e.preventDefault()} onKeyDown={e=>{
  if(e.ctrlKey&&e.shiftKey&&!e.altKey&&shortcutKey(e)==='l'){e.preventDefault();void invoke('popup_menu_lock');return;}
  if(e.key==='Escape'||e.key==='Tab'){e.preventDefault();void dismiss(true);return;}
  const buttons=Array.from(panel.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)')||[]);
  if(!buttons.length)return;
  const current=buttons.indexOf(document.activeElement as HTMLButtonElement);
  let next:number|undefined;
  if(e.key==='ArrowDown')next=(current+1)%buttons.length;
  else if(e.key==='ArrowUp')next=(current-1+buttons.length)%buttons.length;
  else if(e.key==='Home')next=0;
  else if(e.key==='End')next=buttons.length-1;
  if(next!==undefined){e.preventDefault();buttons[next]?.focus();buttons[next]?.scrollIntoView({block:'nearest'});}
 }}>
 {menu?.items.map((item,index)=>item?<button type="button" role="menuitem" aria-label={item.text.split('\t')[0]} key={index} disabled={!item.enabled} onClick={()=>void select(index)}><span>{item.text.split('\t')[0]}</span>{item.text.includes('\t')&&<kbd aria-hidden="true">{item.text.split('\t').slice(1).join(' ')}</kbd>}</button>:<div key={index} role="separator" className="nexus-popup-separator"/>)}
 {error&&<span role="alert">{error}</span>}
 </div>;
}
