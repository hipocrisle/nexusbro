(()=>{
 const visible=e=>!e.disabled&&!e.readOnly&&e.getClientRects().length&&getComputedStyle(e).visibility!=='hidden';
 const formData=form=>{
  const fields=[...form.querySelectorAll('input[type="password"]')].filter(visible),values=fields.map(e=>e.value).filter(Boolean);
  if(!values.length||values.some(v=>v!==values[0]))throw Error('Нет однозначного пароля в форме');
  const text=[...form.querySelectorAll('input')].filter(e=>visible(e)&&['text','email','tel'].includes(e.type)&&e.autocomplete!=='one-time-code'&&!/captcha|otp|verification/i.test(e.name));
  const login=form.querySelector('input[autocomplete~="username"]')||text.find(e=>e.type==='email')||text.filter(e=>e.compareDocumentPosition(fields[0])&Node.DOCUMENT_POSITION_FOLLOWING).at(-1);
  const username=login?.value??'';if(username.length>512||values[0].length>4096)throw Error('Введите логин и пароль в форму');
  return {username,password:values[0]};
 };
 if(__INSTALL__){
  if(window.__nbPasswordHook)return true;window.__nbPasswordHook=true;let last=0;
  document.addEventListener('submit',event=>{
   if(!event.isTrusted||!(event.target instanceof HTMLFormElement)||Date.now()-last<3000)return;
   try{const result=formData(event.target);last=Date.now();window.__nexusbroPasswordCandidate(JSON.stringify(result));}catch{}
  },true);return true;
 }
 const focused=document.activeElement?.closest('form');
 const forms=focused?[focused]:[...document.forms].filter(f=>[...f.querySelectorAll('input[type="password"]')].some(e=>visible(e)&&e.value));
 if(forms.length!==1)throw Error('Выберите форму входа или введите данные вручную');
 return formData(forms[0]);
})()
