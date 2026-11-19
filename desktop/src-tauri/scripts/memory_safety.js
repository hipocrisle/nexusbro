// Evaluated by CefRenderProcessHandler before page scripts in the main world.
// Returns a function held ONLY by native V8Value, never a page-global handle.
(() => {
 'use strict';
 const g=globalThis, d=document, apply=Reflect.apply, construct=Reflect.construct;
 const descriptor=Object.getOwnPropertyDescriptor, define=Object.defineProperty, parent=Object.getPrototypeOf;
 const create=Object.create, stringify=JSON.stringify, finite=Number.isFinite, lower=String.prototype.toLowerCase;
 const textTest=RegExp.prototype.test, riskyTag=/^(?:.*:)?(?:iframe|frame|canvas|audio|video|object|embed)$/;
 const ProxyNative=Proxy, weak=new WeakMap(), weakGet=WeakMap.prototype.get, weakSet=WeakMap.prototype.set;
 const checks=[];let blocked=null, interaction=null, failure=null, mutationBudget=256;
 const mark=reason=>{if(blocked===null)blocked=reason;};
 const fail=reason=>{if(failure===null)failure=reason;};
 const find=(owner,key)=>{for(let p=owner;p;p=parent(p)){const v=descriptor(p,key);if(v)return {owner:p,value:v};}return null;};
 const remember=(owner,key,v,lookup=owner)=>{checks[checks.length]={owner,key,value:v,lookup};};
 const answer=(known,reason,x,y)=>{const out=create(null);out.known=known;out.reason=reason;out.scroll_x=x;out.scroll_y=y;return stringify(out);};
 const proxyFor=(target,reason,before)=>{
  const prior=apply(weakGet,weak,[target]);if(prior)return prior;
  const touch=args=>{if(before)before(args);else mark(reason);};
  const wrapped=new ProxyNative(target,{
   apply(fn,self,args){touch(args);return apply(fn,self,args);},
   construct(fn,args,newTarget){touch(args);return construct(fn,args,newTarget);}
  });
  apply(weakSet,weak,[target,wrapped]);return wrapped;
 };
 const patch=(owner,key,reason,before,constructor=false)=>{
  try{
   const found=find(owner,key);
   if(!found){remember(owner,key,null);return;}
   const item=found.value;
   if(item.value===undefined){remember(found.owner,key,item,owner);return;}
   if(typeof item.value!=='function'){fail('Unsupported native hook');return;}
   const wrapped=proxyFor(item.value,reason,before);
   define(found.owner,key,{...item,value:wrapped});remember(found.owner,key,descriptor(found.owner,key),owner);
   if(constructor&&item.value.prototype){
    const slot=descriptor(item.value.prototype,'constructor');
    if(slot&&slot.value===item.value){define(item.value.prototype,'constructor',{...slot,value:wrapped});remember(item.value.prototype,'constructor',descriptor(item.value.prototype,'constructor'));}
   }
  }catch{fail('Native hooks unavailable');}
 };
 const getter=(owner,key)=>{
  const found=find(owner,key);if(!found||typeof found.value.get!=='function')throw Error('Native getter unavailable');
  const fn=found.value.get;remember(found.owner,key,found.value);return self=>apply(fn,self,[]);
 };
 let observer=null, take=null, walk=null, next=null, matches=null, shadow=null, scrollX=null,scrollY=null;
 let storage=null, storageLength=null, controller=null, designMode=null,nodeType=null, added=null,removed=null,listLength=null,listItem=null,beforeUnload=null;
 try{
  // Capture native operations before any page mutation of prototypes.
  const createTreeWalker=Document.prototype.createTreeWalker, walkerNext=TreeWalker.prototype.nextNode;
  nodeType=getter(Node.prototype,'nodeType');added=getter(MutationRecord.prototype,'addedNodes');removed=getter(MutationRecord.prototype,'removedNodes');listLength=getter(NodeList.prototype,'length');const item=NodeList.prototype.item;listItem=(nodes,i)=>apply(item,nodes,[i]);
  const elementMatches=Element.prototype.matches, shadowGetter=getter(Element.prototype,'shadowRoot');
  walk=root=>apply(createTreeWalker,d,[root,0xffffffff]);next=w=>apply(walkerNext,w,[]);
  matches=n=>apply(elementMatches,n,['input,textarea,select,form,[contenteditable]:not([contenteditable="false"]),audio,video,iframe,frame,canvas,object,embed']);
  shadow=shadowGetter;scrollX=getter(g,'scrollX');scrollY=getter(g,'scrollY');designMode=getter(Document.prototype,'designMode');
  const storageGetter=getter(g,'sessionStorage');storage=()=>storageGetter(g);storageLength=getter(Storage.prototype,'length');
  if(typeof ServiceWorkerContainer!=='undefined'){
   const getServiceWorker=getter(Navigator.prototype,'serviceWorker');const getController=getter(ServiceWorkerContainer.prototype,'controller');
   controller=()=>getController(getServiceWorker(navigator));
  }
  const add=EventTarget.prototype.addEventListener;
  for(const type of ['beforeinput','input','change','paste','drop','compositionstart','pointerdown','click'])
   apply(add,g,[type,event=>{if(event.isTrusted&&interaction===null)interaction='Trusted page interaction or editing';},true]);
  // No per-tab periodic work. takeRecords() also covers mutations queued just
  // before the browser freezes the page for its final probe.
  observer=new MutationObserver(records=>consume(records));
  take=MutationObserver.prototype.takeRecords;
  observer.observe(d,{childList:true,subtree:true});
  for(const name of ['AudioContext','webkitAudioContext','OfflineAudioContext','webkitOfflineAudioContext',
    'RTCPeerConnection','webkitRTCPeerConnection','Worker','SharedWorker','WebSocket','MediaStream','Audio','OffscreenCanvas','SpeechRecognition','webkitSpeechRecognition'])
   patch(g,name,'Media, connection or worker API used',null,true);
  patch(HTMLMediaElement.prototype,'play','Media playback requested');
  if(typeof SpeechSynthesis!=='undefined')patch(SpeechSynthesis.prototype,'speak','Speech playback requested');
  patch(Element.prototype,'attachShadow','Shadow DOM used');
  patch(HTMLCanvasElement.prototype,'getContext','Canvas used');
  patch(HTMLCanvasElement.prototype,'transferControlToOffscreen','Offscreen canvas used');
  if(typeof MediaDevices!=='undefined'){
   patch(MediaDevices.prototype,'getUserMedia','Media capture requested');
   patch(MediaDevices.prototype,'getDisplayMedia','Screen capture requested');
  }
  for(const name of ['getUserMedia','webkitGetUserMedia','mozGetUserMedia'])patch(Navigator.prototype,name,'Media capture requested');
  if(typeof ServiceWorkerContainer!=='undefined')patch(ServiceWorkerContainer.prototype,'register','Service worker registered');
  const creation=index=>args=>{
   if(typeof args[index]!=='string'||args[index].length>128){mark('Uninspectable DOM creation');return;}
   if(apply(textTest,riskyTag,[apply(lower,args[index],[])]))mark('Frame, canvas or media element created');
  };
  patch(d,'createElement',null,creation(0));
  patch(d,'createElementNS',null,creation(1));
  patch(EventTarget.prototype,'addEventListener',null,args=>{if(typeof args[0]!=='string')mark('Uninspectable event registration');else if(args[0]==='beforeunload')mark('Before-unload listener registered');});
  const unload=find(g,'onbeforeunload');
  if(!unload||typeof unload.value.set!=='function')fail('Before-unload hook unavailable');
  else{
   const original=unload.value;
   if(typeof original.get!=='function')fail('Before-unload state unavailable');
   else beforeUnload=()=>apply(original.get,g,[]);
   const setter=new ProxyNative(original.set,{apply(fn,self,args){if(args[0]!=null)mark('Before-unload listener registered');return apply(fn,self,args);}});
   define(unload.owner,'onbeforeunload',{...original,set:setter});remember(unload.owner,'onbeforeunload',descriptor(unload.owner,'onbeforeunload'));
  }
 }catch{fail('Early native instrumentation unavailable');}
 function scan(root,budget){
  if(nodeType(root)===1&&(matches(root)||shadow(root))){mark('Form, media, frame, canvas or shadow DOM');return;}
  const w=walk(root);let n;
  while((n=next(w))){if(--budget.left<0)throw Error('DOM scan limit');if(nodeType(n)===1&&(matches(n)||shadow(n))){mark('Form, media, frame, canvas or shadow DOM');return;}}
 }
 function consume(records){
  // Once protected, observing later mutations is strictly O(1). This is not
  // another cosmetic scanner: dynamic pages exhaust a small lifetime budget
  // and simply stay alive. A fresh requested probe can scan the DOM once.
  if(blocked||failure)return;
  try{
   if(records.length>64||mutationBudget<=0)throw Error('Dynamic document');
   const limit=mutationBudget<64?mutationBudget:64,budget={left:limit};
   for(let i=0;i<records.length;i++){
    for(let side=0;side<2;side++){
     const nodes=side===0?added(records[i]):removed(records[i]);
     for(let j=0;j<listLength(nodes);j++){
      if(--budget.left<0)throw Error('Dynamic document');
      const n=listItem(nodes,j),type=nodeType(n);if(type===1||type===11)scan(n,budget);
      if(blocked)return;
     }
    }
   }
   mutationBudget-=limit-budget.left;
  }catch{fail('Dynamic or uninspectable document');}
 }
 return function memorySafetyProbe(){
  let x=0,y=0;
  try{
   if(failure)return answer(false,failure,x,y);
   // A page may replace a global/method after installation. Never treat an
   // uninstrumented replacement as evidence of absence of media/editing.
   for(let i=0;i<checks.length;i++){
    const c=checks[i],now=descriptor(c.owner,c.key),old=c.value;
    if(old===null){if(find(c.owner,c.key)!==null)throw Error('New native alias');}
    else{const effective=find(c.lookup,c.key);if(!now||!effective||effective.owner!==c.owner||now.value!==old.value||now.get!==old.get||now.set!==old.set)throw Error('Native hook changed');}
   }
   if(observer)consume(apply(take,observer,[]));
   x=scrollX(g);y=scrollY(g);if(!finite(x)||!finite(y))throw Error('Invalid scroll');
   if(!blocked){
    if(beforeUnload&&beforeUnload()!=null)mark('Before-unload handler installed');
    else if(apply(lower,designMode(d),[])==='on')mark('Editable document');
    else if(storageLength(storage())!==0)mark('Session storage is not empty');
    else if(controller&&controller())mark('Service worker controls page');
    else scan(d,{left:4096});
   }
   return answer(failure===null,failure||blocked||interaction,x,y);
  }catch{return answer(false,'Document state unavailable or changed',0,0);}
 };
})()
