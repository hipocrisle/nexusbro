import {useEffect,type RefObject} from 'react';
import {invoke} from '@tauri-apps/api/core';
import {popupMenu} from './nativeMenu';
type Selection={start:number;end:number;value:string};
// The themed owned popup is above CEF; address text and edit callbacks stay here.
export default function AddressMenu({input,onChange,onError,onMenuState}:{input:RefObject<HTMLInputElement>;onChange:(s:string)=>void;onError:(s:string)=>void;onMenuState?:(open:boolean)=>void}){
 useEffect(()=>{
  const e=input.current;if(!e)return;let mounted=true;
  async function edit(action:string,selection:Selection){
   if(!mounted||!e?.isConnected)return;
   try{await invoke('fokus_obolochke');if(!mounted||!e?.isConnected)return;e.focus();if(e.value!==selection.value){e.value=selection.value;onChange(selection.value);}e.setSelectionRange(selection.start,selection.end);
    if(action==='selectAll'){e.select();}
    else if(action==='paste'){const value=await invoke<string>('bufer_chitat');if(!mounted||!e.isConnected)return;document.execCommand('insertText',false,value);onChange(e.value);}
    else if(action==='copy'||action==='cut'){await invoke('bufer_zapisat',{text:selection.value.slice(selection.start,selection.end)});if(action==='cut'&&mounted&&e.isConnected){document.execCommand('delete');onChange(e.value);}}
    else{document.execCommand(action);onChange(e.value);}
   }catch(err){if(mounted)onError(String(err));}
  }
  const open=async(event:MouseEvent)=>{
   event.preventDefault();event.stopPropagation();const selection={start:e.selectionStart??0,end:e.selectionEnd??0,value:e.value};onMenuState?.(true);
   try{await popupMenu(([['undo','Отменить','Ctrl+Z'],['redo','Повторить','Ctrl+Shift+Z'],['cut','Вырезать','Ctrl+X'],['copy','Копировать','Ctrl+C'],['paste','Вставить','Ctrl+V'],['delete','Удалить','Delete'],['selectAll','Выделить всё','Ctrl+A']] as const).map(([action,text,shortcut])=>({text:text+'\t'+shortcut,enabled:!['cut','copy','delete'].includes(action)||selection.start!==selection.end,action:()=>edit(action,selection)})),event.clientX,event.clientY,onError);}
   catch(err){if(mounted)onError(String(err));}finally{if(mounted)onMenuState?.(false);}
  };
  e.addEventListener('contextmenu',open);return()=>{mounted=false;e.removeEventListener('contextmenu',open);};
 },[]);
 return null;
}
