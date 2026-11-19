import {useLayoutEffect,useRef,useState} from 'react';
import QRCode from 'qrcode';

/** Encodes locally; the authenticator secret never enters a URL request or log. */
export default function TotpQr({uri}:{uri:string}){
 const canvas=useRef<HTMLCanvasElement>(null),[ready,setReady]=useState(false),[failed,setFailed]=useState(false);
 useLayoutEffect(()=>{
  const target=canvas.current;if(!target)return;
  setReady(false);setFailed(false);target.width=0;target.height=0;
  try{
   const link=new URL(uri);
   if(link.protocol!=='otpauth:'||link.hostname!=='totp'||!link.searchParams.get('secret')||uri.length>4096)throw new Error();
   const matrix=QRCode.create(uri,{errorCorrectionLevel:'M'}).modules;
   const quiet=4,scale=5,size=(matrix.size+quiet*2)*scale;
   target.width=size;target.height=size;
   const context=target.getContext('2d');if(!context)throw new Error();
   context.fillStyle='#ffffff';context.fillRect(0,0,size,size);context.fillStyle='#000000';
   for(let row=0;row<matrix.size;row++)for(let col=0;col<matrix.size;col++)if(matrix.get(row,col))context.fillRect((col+quiet)*scale,(row+quiet)*scale,scale,scale);
   setReady(true);
  }catch{
   target.width=0;target.height=0;setFailed(true);
  }
  return()=>{target.width=0;target.height=0;};
 },[uri]);
 return <div className="security-totp-qr">
  <canvas ref={canvas} data-testid="totp-qr" role="img" aria-label="QR-код для приложения-аутентификатора" hidden={!ready}/>
  {failed&&<p role="status">QR-код недоступен. Откройте «Ввести ключ вручную» ниже.</p>}
 </div>;
}
