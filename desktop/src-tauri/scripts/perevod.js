(() => {
  const token = __TOKEN__;
  if (window.__nexusbroTranslation) window.__nexusbroTranslation.restore();
  const state = {token, url:location.href, nodes:new Map(), total:0};
  const skip='script,style,noscript,textarea,input,select,option,pre,code,kbd,samp,[contenteditable],[translate="no"],.notranslate';
  const walker=document.createTreeWalker(document.body,NodeFilter.SHOW_TEXT);
  const items=[];let node,chars=0;
  while ((node=walker.nextNode()) && items.length<1000 && chars<300000) {
    const text=node.nodeValue,p=node.parentElement;
    if(!p||p.closest(skip)||!p.getClientRects().length||text.trim().length<3||!/[\p{L}]/u.test(text)||text.length>10000)continue;
    const id=items.length;state.nodes.set(id,{node,original:text,last:text});items.push({id,text});chars+=text.length;
  }
  state.total=items.length;
  state.apply=(t,items)=>{
    if(t!==state.token||location.href!==state.url)return;
    for(const {id,text} of items){const entry=state.nodes.get(id);if(entry&&entry.node.isConnected&&entry.node.nodeValue===entry.last){entry.node.nodeValue=text;entry.last=text;}}
  };
  state.restore=()=>{for(const e of state.nodes.values()){if(e.node.isConnected&&e.node.nodeValue===e.last)e.node.nodeValue=e.original;}state.nodes.clear();if(window.__nexusbroTranslation===state)delete window.__nexusbroTranslation;};
  window.__nexusbroTranslation=state;
  return {token,items};
})()
