// Первый одиночный клик выделяет адрес; последующие клики и drag — обычные.
// ПКМ никогда не должен менять уже выбранный пользователем фрагмент.
export type AddressPointer = {x:number;y:number;selectOnRelease:boolean}|null;
export function addressDown(input:HTMLInputElement,button:number,x:number,y:number):AddressPointer {
  return button===0?{x,y,selectOnRelease:document.activeElement!==input}:null;
}
export function addressUp(input:HTMLInputElement,start:AddressPointer,x:number,y:number,clicks:number):boolean {
  if(start?.selectOnRelease&&clicks===1&&Math.hypot(x-start.x,y-start.y)<4){input.select();return true;}
  return false;
}
export function editableTarget(target:EventTarget|null):boolean {
  return target instanceof Element&&!!target.closest('input,textarea,[contenteditable]:not([contenteditable="false"])');
}
