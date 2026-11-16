"""Проверка подписей всех пакетов и согласованности updater manifest перед публикацией."""
import argparse,base64,hashlib,json,pathlib
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey
p=argparse.ArgumentParser();p.add_argument('--directory',type=pathlib.Path,required=True);p.add_argument('--version',required=True);p.add_argument('--config',type=pathlib.Path,default=pathlib.Path('desktop/src-tauri/tauri.conf.json'));a=p.parse_args()
m=json.loads((a.directory/'latest.json').read_text());assert m['version']==a.version
public=json.loads(a.config.read_text())['plugins']['updater']['pubkey']
key=base64.b64decode(base64.b64decode(public).decode().splitlines()[1]);verify=Ed25519PublicKey.from_public_bytes(key[10:]);proof=[]
for suffix,platform in [('.exe','windows-x86_64'),('.deb','linux-x86_64-deb'),('.rpm','linux-x86_64-rpm')]:
 files=list(a.directory.glob('*'+suffix));assert len(files)==1,files
 f=files[0];assert a.version in f.name
 encoded=pathlib.Path(str(f)+'.sig').read_text().strip();assert m['platforms'][platform]['signature']==encoded
 assert m['platforms'][platform]['url'].endswith('/v'+a.version+'/'+f.name)
 lines=base64.b64decode(encoded).decode().splitlines();sig=base64.b64decode(lines[1]);assert sig[:2]==b'ED' and sig[2:10]==key[2:10]
 digest=hashlib.blake2b();sha=hashlib.sha256()
 with f.open('rb') as stream:
  while chunk:=stream.read(1024*1024):digest.update(chunk);sha.update(chunk)
 verify.verify(sig[10:],digest.digest());verify.verify(base64.b64decode(lines[3]),sig[10:]+lines[2].removeprefix('trusted comment: ').encode())
 proof.append({'file':f.name,'bytes':f.stat().st_size,'sha256':sha.hexdigest(),'ed25519_payload_and_comment_verified':True,'manifest_signature_matches':True})
assert m['platforms']['linux-x86_64']==m['platforms']['linux-x86_64-deb']
out={'version':a.version,'packages':proof}
(a.directory/'signature-verification.json').write_text(json.dumps(out,indent=2)+'\n');print(json.dumps(out,indent=2))
