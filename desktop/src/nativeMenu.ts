import {invoke} from '@tauri-apps/api/core';
export type NativeEntry={text:string;enabled?:boolean;action:()=>void|Promise<unknown>}|null;
const colors=['--fon','--fon2','--fon3','--tekst','--tiho','--kraj','--akcent'] as const;
function rgb(value:string):number[]{
 const v=value.trim();
 if(/^#[0-9a-f]{6}$/i.test(v))return [1,3,5].map(i=>parseInt(v.slice(i,i+2),16));
 if(/^#[0-9a-f]{3}$/i.test(v))return [1,2,3].map(i=>parseInt(v[i]+v[i],16));
 const match=v.match(/^rgba?\(\s*([\d.]+)[, ]+([\d.]+)[, ]+([\d.]+)/i);
 return match?match.slice(1,4).map(n=>Math.max(0,Math.min(255,Math.round(Number(n))))):[20,25,35];
}
// Only safe labels/theme cross IPC; executable actions stay in the owner window.
export async function popupMenu(items:NativeEntry[],x:number,y:number,onError:(message:string)=>void){
 const root=getComputedStyle(document.documentElement),body=getComputedStyle(document.body);
 const theme={colors:Object.fromEntries(colors.map(k=>[k,rgb(root.getPropertyValue(k))])),font_family:body.fontFamily,font_size:Math.max(10,Math.min(24,parseFloat(body.fontSize)||14))};
 const selected=await invoke<number|null>('popup_menu_open',{items:items.map(item=>item?{text:item.text,enabled:item.enabled!==false}:null),theme,x,y});
 if(selected===null)return;
 const item=items[selected];if(!item||item.enabled===false)return;
 try{await item.action();}catch(error){onError(String(error));}
}
