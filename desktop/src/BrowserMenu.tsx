import {useState,type ReactNode} from 'react';
import {popupMenu} from './nativeMenu';
export type MenuItem={label:string;shortcut?:string;disabled?:boolean;keepsHidden?:boolean;action:()=>void|Promise<unknown>}|null;
export default function BrowserMenu({label,children,items,className=''}:{label:string;children:ReactNode;items:MenuItem[];className?:string}){
 const [error,setError]=useState(''),[open,setOpen]=useState(false);
 return <><button type="button" className={'browser-menu-button '+className} title={label} aria-label={label} aria-haspopup="menu" aria-expanded={open} onClick={async e=>{
  const r=e.currentTarget.getBoundingClientRect();setError('');setOpen(true);
  try{await popupMenu(items.map(item=>item?{text:item.label+(item.shortcut?'\t'+item.shortcut:''),enabled:!item.disabled,action:item.action}:null),r.left,r.bottom+4,setError);}catch(e){setError(String(e));}finally{setOpen(false);}
 }}>{children}</button>{error&&<span role="alert" className="oshibka-navigacii" title={error}>{error}</span>}</>;
}
