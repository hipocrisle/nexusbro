import {invoke} from '@tauri-apps/api/core';
let opening:string|null=null;
const dialogs=()=>[...new Set(Array.from(document.querySelectorAll<HTMLElement>('[role="dialog"],.zatemnenie')).map(node=>node.matches('[role="dialog"]')?node:node.querySelector<HTMLElement>('[role="dialog"],.dialog')||node))].filter(node=>node.getClientRects().length>0);
/** Serialize the async hide-before-render gap; never discard an existing form. */
export function claimModal(owner:string,ownSelector?:string){
 if(opening)return false;
 if(dialogs().some(node=>!ownSelector||!node.matches(ownSelector)))return false;
 opening=owner;return true;
}
export function settleModal(owner:string){if(opening===owner)opening=null;}
export function renderedModal(owner:string){requestAnimationFrame(()=>settleModal(owner));}
export async function restoreModalContent(){await new Promise<void>(resolve=>requestAnimationFrame(()=>resolve()));if(!opening&&!dialogs().length)await invoke('pokazat_dvizhki',{vidno:true});}
