import {invoke} from '@tauri-apps/api/core';
import {emit} from '@tauri-apps/api/event';
export type VaultStatus={exists:boolean;unlocked:boolean};
export type CredentialSummary={id:string;origin:string;username:string;title:string;super_id:string;updated_at:number};
export type CredentialDraft={draft_id:string;origin:string;username:string};
export type AccountStatus={configured:boolean;logged_in:boolean;username:string;server:string;totp_enabled:boolean;user_id?:string|null};
export type SyncSelection={settings:boolean;bookmarks:boolean;supers:string[];passwords:string[]};
export type SyncReport={pushed:number;pulled:number;conflicts:string[];errors?:string[]};
export type SyncRecordState={state:'off'|'pending'|'ok'|'error'|'conflict';last_success?:number};
export type SyncStatus={selection:SyncSelection;last_sync?:number|string|null;available_supers:{id:string;name:string}[];report?:SyncReport;super_states?:Record<string,SyncRecordState>;sync_state?:SyncRecordState};
export const emptySelection=():SyncSelection=>({settings:false,bookmarks:false,supers:[],passwords:[]});
export const SECURITY_STATE_CHANGED='nexusbro-security-state-changed';
async function securityInvoke<T>(area:'vault'|'passwords'|'account'|'sync',action:string,args:Record<string,unknown>){
 const notify=area==='vault'?action!=='status':area==='account'?action!=='status':area==='sync'?action!=='status':false;
 const opId=area==='sync'&&action==='run'?crypto.randomUUID():undefined;
 const announce=(detail:Record<string,unknown>)=>{window.dispatchEvent(new CustomEvent(SECURITY_STATE_CHANGED,{detail}));void emit(SECURITY_STATE_CHANGED,detail).catch(()=>{});};
 if(opId)announce({area,action,phase:'start',opId});
 let failed=false;
 try{return await invoke<T>('security_'+area,{action,...args});}
 catch(error){failed=true;throw error;}
 finally{if(notify){announce({area,action,failed,...(opId?{phase:'end',opId}:{})});}}
}
export const security={
 vault:(action:string,password?:string)=>securityInvoke<VaultStatus>('vault',action,password===undefined?{}:{password}),
 passwords:<T=CredentialSummary[]>(action:string,data?:Record<string,unknown>)=>securityInvoke<T>('passwords',action,data?{data}:{}),
 account:<T=AccountStatus>(action:string,data?:Record<string,unknown>)=>securityInvoke<T>('account',action,data?{data}:{}),
 sync:<T=SyncStatus>(action:string,data?:Record<string,unknown>)=>securityInvoke<T>('sync',action,data?{data}:{}),
};
export async function currentCredentialTarget(){
 const super_id=await invoke<string|null>('aktivnaya');if(!super_id)throw new Error('Выберите обычную супер-вкладку с открытой страницей.');
 const session=await invoke<{aktivnaya:number;vkladki:{id:number;privatnaya?:boolean;otlozhena?:boolean}[]}>('obychnye_spisok',{id:super_id});
 const tab=session?.vkladki?.find(v=>v.id===session.aktivnaya);
 if(!tab||tab.otlozhena)throw new Error('Сначала откройте страницу входа.');
 if(tab.privatnaya)throw new Error('Пароли приватных вкладок не сохраняются и не заполняются.');
 return {super_id,tab_id:tab.id};
}
