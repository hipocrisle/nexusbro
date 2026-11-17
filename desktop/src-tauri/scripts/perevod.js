(() => {
  const token = __TOKEN__;
  if (window.__nexusbroTranslation) window.__nexusbroTranslation.restore();
  // Якорь Wikipedia не меняет документ: незавершённый перевод должен
  // продолжаться. Путь и query остаются частью защиты от устаревших ответов.
  const pageUrl=()=>location.href.split('#',1)[0];
  const state = {token, url:pageUrl(), nodes:new Map(), total:0};
  const skip='script,style,noscript,textarea,input,select,option,pre,code,kbd,samp,[contenteditable],[translate="no"],.notranslate';
  const walker=document.createTreeWalker(document.body,NodeFilter.SHOW_TEXT);
  const candidates=[];let node;
  // Сначала видимый читателю текст статьи, затем остальная страница. Большое
  // меню Wikipedia больше не съедает весь лимит перед первым абзацем.
  while ((node=walker.nextNode()) && candidates.length<4000) {
    const text=node.nodeValue,p=node.parentElement;
    if(!p||p.closest(skip)||!p.getClientRects().length||text.trim().length<3||!/[\p{L}]/u.test(text)||text.length>10000)continue;
    const style=getComputedStyle(p);if(style.visibility==='hidden'||style.visibility==='collapse'||style.opacity==='0')continue;
    const rect=p.getBoundingClientRect(),onscreen=rect.bottom>0&&rect.top<innerHeight&&rect.right>0&&rect.left<innerWidth;
    const article=!!p.closest('main,article,[role="main"],#mw-content-text');
    candidates.push({node,text,rank:onscreen?(article?0:1):(article?2:3)});
  }
  candidates.sort((a,b)=>a.rank-b.rank);
  const items=[];let chars=0;
  for(const {node,text} of candidates){
    if(items.length>=1000||chars+text.length>300000)break;
    const id=items.length;state.nodes.set(id,{node,original:text,last:text});items.push({id,text});chars+=text.length;
  }
  state.total=items.length;
  state.apply=(t,items)=>{
    if(t!==state.token||pageUrl()!==state.url)return;
    for(const {id,text} of items){const entry=state.nodes.get(id);if(entry&&entry.node.isConnected&&entry.node.nodeValue===entry.last){// Не склеивать слова вокруг ссылок/inline-элементов после нормализации
      // whitespace сетевым пакетом. Исходные отступы принадлежат DOM-узлу.
      const leading=entry.original.match(/^\s*/)[0],trailing=entry.original.match(/\s*$/)[0];
      entry.last=leading+text.trim()+trailing;entry.node.nodeValue=entry.last;}}
  };
  state.restore=()=>{for(const e of state.nodes.values()){if(e.node.isConnected&&e.node.nodeValue===e.last)e.node.nodeValue=e.original;}state.nodes.clear();if(window.__nexusbroTranslation===state)delete window.__nexusbroTranslation;};
  window.__nexusbroTranslation=state;
  return {token,items};
})()
