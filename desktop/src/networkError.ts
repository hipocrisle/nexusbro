/** Chromium network codes only; no speculative claim that a website is down globally. */
export function networkError(code:number){
 if(code===-105||code===-137||code===-800)return 'Не удалось определить адрес сайта (DNS).';
 if(code===-130||code===-111||code===-115||code===-127)return 'Не удалось подключиться через выбранный прокси или VPN.';
 if(code<=-200&&code>=-299)return 'Не удалось проверить сертификат сайта. Защита соединения не отключалась.';
 if(code===-107||code===-113||code===-117)return 'Не удалось установить защищённое соединение с сайтом (TLS).';
 if(code===-106)return 'Нет подключения к сети.';
 if(code===-118||code===-7)return 'Истекло время ожидания ответа по выбранному пути.';
 if(code===-102)return 'Соединение отклонено сайтом или узлом выбранного пути.';
 if(code===-101||code===-100)return 'Соединение с сайтом прервано.';
 return `Страница не загрузилась. Сетевой код Chromium: ${code}.`;
}
