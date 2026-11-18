import {invoke} from '@tauri-apps/api/core';
export type VaultStatus={exists:boolean;unlocked:boolean};
export type CredentialSummary={id:string;origin:string;username:string;title:string;super_id:string;updated_at:number};
export type CredentialDraft={draft_id:string;origin:string;username:string};
export type AccountStatus={configured:boolean;logged_in:boolean;username:string;server:string;totp_enabled:boolean};
export type SyncSelection={settings:boolean;bookmarks:boolean;supers:string[];passwords:string[]};
export type SyncReport={pushed:number;pulled:number;conflicts:string[];errors?:string[]};
export type SyncStatus={selection:SyncSelection;last_sync?:number|string|null;available_supers:{id:string;name:string}[];report?:SyncReport};
export const emptySelection=():SyncSelection=>({settings:false,bookmarks:false,supers:[],passwords:[]});
export const security={
 vault:(action:string,password?:string)=>invoke<VaultStatus>('security_vault',{action,...(password===undefined?{}:{password})}),
 passwords:<T=CredentialSummary[]>(action:string,data?:Record<string,unknown>)=>invoke<T>('security_passwords',{action,...(data?{data}:{})}),
 account:<T=AccountStatus>(action:string,data?:Record<string,unknown>)=>invoke<T>('security_account',{action,...(data?{data}:{})}),
 sync:<T=SyncStatus>(action:string,data?:Record<string,unknown>)=>invoke<T>('security_sync',{action,...(data?{data}:{})}),
};
export async function currentCredentialTarget(){
 const super_id=await invoke<string|null>('aktivnaya');if(!super_id)throw new Error('Выберите обычную супер-вкладку с открытой страницей.');
 const session=await invoke<{aktivnaya:number;vkladki:{id:number;privatnaya?:boolean;otlozhena?:boolean}[]}>('obychnye_spisok',{id:super_id});
 const tab=session?.vkladki?.find(v=>v.id===session.aktivnaya);
 if(!tab||tab.otlozhena)throw new Error('Сначала откройте страницу входа.');
 if(tab.privatnaya)throw new Error('Пароли приватных вкладок не сохраняются и не заполняются.');
 return {super_id,tab_id:tab.id};
}
