import {getCurrentWindow} from '@tauri-apps/api/window';
import type {EventCallback,EventName} from '@tauri-apps/api/event';

// Глобальный listen(Any) получает даже emit_to для другого окна.
// Состояние вкладок и команды интерфейса принадлежат текущему окну.
export function listenWindow<T>(event:EventName,handler:EventCallback<T>){
 return getCurrentWindow().listen<T>(event,handler);
}
