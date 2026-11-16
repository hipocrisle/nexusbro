"""Проверка реального Xray с GeoIP/GeoSite и графом подписки, без выхода в сеть."""
import argparse,json,os,pathlib,subprocess,tempfile
p=argparse.ArgumentParser();p.add_argument('--bin',required=True);a=p.parse_args()
binary=pathlib.Path(a.bin).resolve()
for f in ['geoip.dat','geosite.dat']:assert (binary.parent/f).is_file(),f
config={'inbounds':[{'listen':'127.0.0.1','port':10808,'protocol':'socks','settings':{'auth':'noauth','udp':False}}],
 'outbounds':[{'tag':'proxy-1','protocol':'vless','settings':{'vnext':[{'address':'127.0.0.1','port':1,'users':[{'id':'11111111-2222-3333-4444-555555555555','encryption':'none'}]}]}},{'tag':'direct','protocol':'freedom'},{'tag':'block','protocol':'blackhole'}],
 'routing':{'rules':[{'type':'field','ip':['geoip:private'],'outboundTag':'block'},{'type':'field','domain':['geosite:category-ads-all'],'outboundTag':'block'},{'type':'field','network':'tcp,udp','balancerTag':'auto'}], 'balancers':[{'tag':'auto','selector':['proxy-'],'strategy':{'type':'leastPing'}}]},
 'burstObservatory':{'subjectSelector':['proxy-'],'pingConfig':{'destination':'http://127.0.0.1/','interval':'4m','timeout':'2s','sampling':3,'httpMethod':'HEAD'}}}
with tempfile.TemporaryDirectory() as d:
 f=pathlib.Path(d)/'fixture.json';f.write_text(json.dumps(config))
 r=subprocess.run([str(binary),'run','-test','-c',str(f)],env={**os.environ,'XRAY_LOCATION_ASSET':str(binary.parent)},capture_output=True,text=True,timeout=20)
 assert r.returncode==0,r.stdout+r.stderr
print('Xray: shipped GeoIP + GeoSite + subscription balancer validated (no network)')
