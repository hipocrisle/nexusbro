// Complete only an address prefix, never a title match or a search phrase.
export function addressCompletion(typed:string,rows:{adres:string}[]):{value:string;from:number}|null{
 if(typed.length<2||typed.trim()!==typed||/\s/.test(typed))return null;
 const query=typed.toLocaleLowerCase();
 for(const row of rows){
  if(!/^https?:\/\//i.test(row.adres))continue;
  const url=row.adres,lower=url.toLocaleLowerCase();
  if(lower.startsWith(query)&&url.length>typed.length)return {value:url,from:typed.length};
  if(!/^[a-z][a-z0-9+.-]*:/i.test(typed)){
   const protocol=lower.startsWith('https://')?8:7;
   const www=lower.slice(protocol).startsWith('www.')&&!query.startsWith('www.')?4:0;
   if(lower.slice(protocol+www).startsWith(query)&&url.length>protocol+www+typed.length)return {value:url,from:protocol+www+typed.length};
  }
 }
 return null;
}
