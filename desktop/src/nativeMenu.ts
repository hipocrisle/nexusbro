import {Menu} from '@tauri-apps/api/menu';
import {LogicalPosition} from '@tauri-apps/api/dpi';
import {getCurrentWindow} from '@tauri-apps/api/window';
export type NativeEntry={text:string;enabled?:boolean;action:()=>void|Promise<unknown>}|null;
let sequence=0;
const live=new Map<string,Menu>();
// A native popup is composited over child CEF windows. Never hide the document.
export async function popupMenu(items:NativeEntry[],x:number,y:number,onError:(message:string)=>void){
 const label=getCurrentWindow().label;const previous=live.get(label);if(previous){live.delete(label);await previous.close();}
 const prefix='nb-menu-'+label+'-'+(++sequence);
 const menu=await Menu.new({items:items.map((item,i)=>item?{id:prefix+'-'+i,text:item.text,enabled:item.enabled,action:()=>{Promise.resolve().then(item.action).catch(e=>onError(String(e)));}}:{item:'Separator' as const})});
 live.set(label,menu);
 // popup() resolves after presentation on GTK, not necessarily after dismissal.
 // Keep its native resource/callbacks alive until the next popup or window teardown.
 try{await menu.popup(new LogicalPosition(x,y),getCurrentWindow());}catch(error){if(live.get(label)===menu)live.delete(label);await menu.close().catch(()=>{});throw error;}
}
