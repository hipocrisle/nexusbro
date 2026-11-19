"""Publish an existing tagged build only after a fresh native runtime check."""
import datetime,json,os,pathlib,shutil,subprocess,sys
repo=os.environ['REPO'];tag=os.environ['TAG'];run=os.environ['BUILD_RUN'];version=tag.removeprefix('v')
def gh(*args):return subprocess.check_output(['gh',*args,'--repo',repo],text=True)
metadata=json.loads(gh('run','view',run,'--json','status,headSha,jobs'))
assert metadata['status']=='completed'
ref=json.loads(subprocess.check_output(['gh','api',f'repos/{repo}/git/ref/tags/{tag}'],text=True))
assert ref['object']['type']=='commit' and ref['object']['sha']==metadata['headSha'],'Build does not match immutable release tag'
def linux_accepted(jobs,build_run,runtime_build,runtime_result):
 # runtime_result is a needs.* result supplied by this workflow, never an input.
 if runtime_build:return runtime_build==build_run and runtime_result=='success'
 return any(j['conclusion']=='success' and 'ubuntu-24.04' in j['name'] for j in jobs)
assert linux_accepted(metadata['jobs'],run,os.environ.get('LINUX_RUNTIME_BUILD',''),os.environ.get('LINUX_RUNTIME_RESULT','')),'Missing Linux acceptance of the exact candidate binary'
# The replacement job verifies this too; retain an independent publication gate.
if os.environ.get('LINUX_RUNTIME_BUILD'):
 assert os.environ['LINUX_RUNTIME_BUILD']==run and os.environ.get('LINUX_RUNTIME_RESULT')=='success','Linux runtime check did not accept this build'
 subprocess.run(['git','fetch','--no-tags','origin',metadata['headSha']],check=True)
 subprocess.run(['git','diff','--exit-code',metadata['headSha'],'HEAD','--','desktop'],check=True)

root=pathlib.Path('.dev-check/publish');root.mkdir(parents=True,exist_ok=True)
for name in ['nexusbro-windows','nexusbro-linux']:
 subprocess.run(['gh','run','download',run,'--repo',repo,'-n',name,'--dir',str(root/name)],check=True)
files={}
for suffix in ['.exe','.deb','.rpm']:
 matches=list(root.rglob('*'+suffix));assert len(matches)==1,matches
 payload=matches[0];sig=pathlib.Path(str(payload)+'.sig');assert sig.is_file()
 assert version in payload.name
 shutil.copy2(payload,root/payload.name);shutil.copy2(sig,root/sig.name);files[suffix]=root/payload.name
base=f'https://github.com/{repo}/releases/download/{tag}/'
platforms={}
for suffix,key in [('.exe','windows-x86_64'),('.deb','linux-x86_64-deb'),('.rpm','linux-x86_64-rpm')]:
 f=files[suffix];platforms[key]={'signature':pathlib.Path(str(f)+'.sig').read_text().strip(),'url':base+f.name}
platforms['linux-x86_64']=platforms['linux-x86_64-deb']
(root/'latest.json').write_text(json.dumps({'version':version,'notes':f'NexusBro {version}','pub_date':datetime.datetime.now(datetime.timezone.utc).isoformat(),'platforms':platforms},indent=2)+'\n')
subprocess.run([sys.executable,'tests/release_signatures.py','--directory',str(root),'--version',version],check=True)
release=json.loads(gh('release','view',tag,'--json','isDraft'));assert release['isDraft'],'Refuse replacing a published release'
assets=[str(x) for f in files.values() for x in [f,pathlib.Path(str(f)+'.sig')]]
subprocess.run(['gh','release','upload',tag,*assets,str(root/'latest.json'),str(root/'signature-verification.json'),'--repo',repo,'--clobber'],check=True)
subprocess.run(['gh','release','edit',tag,'--repo',repo,'--notes-file',f'docs/RELEASE-{version}.md','--draft=false','--latest'],check=True)
