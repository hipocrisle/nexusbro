import {useEffect,useRef,type RefObject} from 'react';
import {Menu} from '@tauri-apps/api/menu';
import {LogicalPosition} from '@tauri-apps/api/dpi';
import {getCurrentWindow} from '@tauri-apps/api/window';
import {invoke} from '@tauri-apps/api/core';
type Selection={start:number;end:number;value:string};
// Нативное меню рисуется поверх CEF: страницу не скрываем даже на мгновение.
export default function AddressMenu({input,onChange,onError,onMenuState}:{input:RefObject<HTMLInputElement>;onChange:(s:string)=>void;onError:(s:string)=>void;onMenuState?:(open:boolean)=>void}){
 const menu=useRef<Menu|null>(null);
 useEffect(()=>{
  const e=input.current;if(!e)return;
  async function edit(action:string,selection:Selection){
   try{await invoke('fokus_obolochke');e!.focus();if(e!.value!==selection.value){e!.value=selection.value;onChange(selection.value);}e!.setSelectionRange(selection.start,selection.end);
    if(action==='selectAll'){e!.select();}
    else if(action==='paste'){const value=await invoke<string>('bufer_chitat');document.execCommand('insertText',false,value);onChange(e!.value);}
    else if(action==='copy'||action==='cut'){await invoke('bufer_zapisat',{text:selection.value.slice(selection.start,selection.end)});if(action==='cut'){document.execCommand('delete');onChange(e!.value);}}
    else{document.execCommand(action);onChange(e!.value);}
   }catch(err){onError(String(err));}
   finally{onMenuState?.(false);}
  }
  const open=async(event:MouseEvent)=>{
   event.preventDefault();event.stopPropagation();const selection={start:e.selectionStart??0,end:e.selectionEnd??0,value:e.value};const x=event.clientX,y=event.clientY;onMenuState?.(true);
   try{if(menu.current){await menu.current.close();menu.current=null;}
    const prefix='address-'+getCurrentWindow().label+'-'+Date.now();
    menu.current=await Menu.new({items:([['undo','Отменить','Ctrl+Z'],['redo','Повторить','Ctrl+Shift+Z'],['cut','Вырезать','Ctrl+X'],['copy','Копировать','Ctrl+C'],['paste','Вставить','Ctrl+V'],['delete','Удалить','Delete'],['selectAll','Выделить всё','Ctrl+A']] as const).map(([id,text,accelerator])=>({id:prefix+'-'+id,text,accelerator,enabled:!['cut','copy','delete'].includes(id)||selection.start!==selection.end,action:()=>void edit(id,selection)}))});
    await menu.current.popup(new LogicalPosition(x,y),getCurrentWindow());
   }catch(err){onError(String(err));}finally{onMenuState?.(false);}
  };
  e.addEventListener('contextmenu',open);return()=>{e.removeEventListener('contextmenu',open);if(menu.current)void menu.current.close();};
 },[]);
 return null;
}
