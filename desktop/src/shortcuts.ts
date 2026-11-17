// Физическая клавиша работает при любой раскладке. RDP/виртуальные клавиатуры
// иногда не передают code: для русской раскладки есть эквивалентный запасной путь.
const ru='йцукенгшщзфывапролдячсмить';
const en='qwertyuiopasdfghjklzxcvbnm';
export function shortcutKey(e:Pick<KeyboardEvent,'code'|'key'>):string {
 if(/^Key[A-Z]$/.test(e.code))return e.code.slice(3).toLowerCase();
 const key=e.key.toLowerCase(),index=ru.indexOf(key);
 return index>=0?en[index]:key;
}
