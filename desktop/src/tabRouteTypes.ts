export type SelectableTabRoute = {kind:'inherit'}|{kind:'direct'}|{kind:'connection';id:string};
export type TabRoute = SelectableTabRoute|{kind:'blocked';reason:string};
export type TabRouteState = {route?:TabRoute;route_generation?:number;route_pending?:boolean;route_error?:string|null};
export type TabRouteChoices = {inheritLabel:string;connections:{id:string;name:string}[]};
export type TabRouteTarget = TabRouteState & {id:number;zagolovok:string;memory_pending?:boolean};
export const validRouteId=(value:string)=>/^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$/.test(value);
export const routeStateKnown=(value:TabRouteState)=>!!value.route&&Number.isSafeInteger(value.route_generation)&&value.route_generation!>=0;
export function routeKey(route?:TabRoute){
 if(route?.kind==='inherit'||route?.kind==='direct')return route.kind;
 if(route?.kind==='connection'&&validRouteId(route.id))return 'connection:'+route.id;
 return 'blocked';
}
export function routeFromKey(value:string):SelectableTabRoute|null{
 if(value==='inherit'||value==='direct')return {kind:value};
 if(value.startsWith('connection:')&&validRouteId(value.slice(11)))return {kind:'connection',id:value.slice(11)};
 return null;
}
export function routeLabel(route:TabRoute|undefined,choices:TabRouteChoices|null){
 if(!route)return 'Путь ещё не подтверждён';
 if(route.kind==='inherit')return choices?'Как у супервкладки: '+choices.inheritLabel:'Как у супервкладки';
 if(route.kind==='direct')return 'Прямое соединение';
 if(route.kind==='connection')return choices?.connections.find(item=>item.id===route.id)?.name??'Сохранённое подключение';
 return 'Сохранённый путь недоступен';
}
export function checkedRouteChoices(value:TabRouteChoices):TabRouteChoices{
 if(!value||typeof value.inheritLabel!=='string'||!Array.isArray(value.connections)||value.connections.length>100)throw Error('Не удалось получить список путей.');
 const ids=new Set<string>();
 for(const item of value.connections){
  if(!item||typeof item.id!=='string'||!validRouteId(item.id)||ids.has(item.id)||typeof item.name!=='string'||!item.name.trim())throw Error('Список подключений некорректен. Проверьте настройки подключений.');
  ids.add(item.id);
 }
 return {inheritLabel:value.inheritLabel,connections:value.connections.map(item=>({id:item.id,name:item.name}))};
}
