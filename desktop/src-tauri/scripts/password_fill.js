(()=>{
 if(location.origin!==__ORIGIN__||window.top!==window)throw Error('Document changed');
 const credentials=[__USERNAME__,__PASSWORD__];
 const visible=e=>!e.disabled&&!e.readOnly&&e.getClientRects().length&&getComputedStyle(e).visibility!=='hidden';
 const fields=[...document.querySelectorAll('input[type=password]')].filter(e=>visible(e)&&e.autocomplete!=='new-password');
 if(fields.length!==1||!fields[0].form)throw Error('Ambiguous form');
 const p=fields[0],text=[...p.form.querySelectorAll('input')].filter(e=>visible(e)&&['text','email','tel'].includes(e.type)&&e.autocomplete!=='one-time-code'&&!/captcha|otp|verification/i.test(e.name));
 const u=text.find(e=>e.autocomplete.split(/\s+/).includes('username'))||text.find(e=>e.type==='email')||text.filter(e=>e.compareDocumentPosition(p)&Node.DOCUMENT_POSITION_FOLLOWING).at(-1);
 const declared=p.form.querySelector('input[autocomplete~="username"]');
 if(!u&&declared?.value&&declared.value!==credentials[0])throw Error('Different account selected');
 const set=Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set;
 const pairs=[[p,credentials[1]]];if(u)pairs.unshift([u,credentials[0]]);
 for(const [e,value] of pairs){set.call(e,value);e.dispatchEvent(new Event('input',{bubbles:true}));e.dispatchEvent(new Event('change',{bubbles:true}));}
 return true;
})()
