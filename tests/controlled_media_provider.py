"""Verify provenance and unchanged generation recipe of an owned media artifact.
Reads GitHub metadata and workflow source only; never downloads a binary.
"""
import argparse,base64,hashlib,json,re,subprocess
from pathlib import Path

def recipe(text):
 block=text.split('\n  youtube-media-fixture:',1)[1].split('\n  build:',1)[0]
 commands=[line.strip() for line in block.splitlines() if line.strip().startswith('ffmpeg -hide_banner ')]
 assert len(commands)==3,'controlled_media_recipe_missing'
 return '\n'.join(commands)

def request_run(text):
 assert isinstance(text,str) and len(text)<=512,'controlled_media_request_invalid'
 value=json.loads(text)
 assert type(value) is dict and set(value)=={'controlled_media_build'} and type(value['controlled_media_build']) is str,'controlled_media_request_invalid'
 return value['controlled_media_build']

def verify(run_id,repository):
 assert re.fullmatch(r'[1-9][0-9]{0,19}',run_id),'controlled_media_run_invalid'
 assert repository=='hipocrisle/nexusbro','controlled_media_repository_invalid'
 def api(path):
  raw=subprocess.check_output(['gh','api','repos/'+repository+'/'+path],timeout=45)
  assert len(raw)<=2*1024*1024,'controlled_media_metadata_bound'
  return json.loads(raw)
 run=api('actions/runs/'+run_id)
 assert run.get('head_repository',{}).get('full_name')==repository,'controlled_media_foreign_source'
 assert run.get('event') in ('push','workflow_dispatch'),'controlled_media_untrusted_event'
 assert run.get('path')=='.github/workflows/build.yml','controlled_media_wrong_workflow'
 source=run.get('head_sha','');assert re.fullmatch(r'[0-9a-f]{40}',source),'controlled_media_source_invalid'
 jobs=api('actions/runs/'+run_id+'/jobs?filter=all&per_page=100').get('jobs',[])
 assert any(j.get('name')=='youtube-media-fixture' and j.get('conclusion')=='success' for j in jobs),'controlled_media_job_not_verified'
 artifacts=api('actions/runs/'+run_id+'/artifacts?per_page=100').get('artifacts',[])
 matches=[v for v in artifacts if v.get('name')=='youtube-controlled-media' and not v.get('expired')]
 assert len(matches)==1 and re.fullmatch(r'sha256:[0-9a-f]{64}',matches[0].get('digest','')),'controlled_media_artifact_unverified'
 old=api('contents/.github/workflows/build.yml?ref='+source)
 assert old.get('encoding')=='base64','controlled_media_source_encoding'
 previous=base64.b64decode(''.join(old['content'].splitlines()),validate=True).decode()
 current=(Path(__file__).resolve().parents[1]/'.github/workflows/build.yml').read_text()
 old_recipe=recipe(previous);assert old_recipe==recipe(current),'controlled_media_recipe_changed'
 return {'schema':1,'provider_run':int(run_id),'provider_source':source,'artifact_id':matches[0]['id'],
         'artifact_digest':matches[0]['digest'],'recipe_sha256':hashlib.sha256(old_recipe.encode()).hexdigest(),'provenance_verified':True}

if __name__=='__main__':
 p=argparse.ArgumentParser();g=p.add_mutually_exclusive_group(required=True);g.add_argument('--run');g.add_argument('--request');p.add_argument('--repository',required=True);p.add_argument('--out',required=True);a=p.parse_args()
 try:
  result=verify(request_run(a.request) if a.request is not None else a.run,a.repository);Path(a.out).parent.mkdir(parents=True,exist_ok=True);Path(a.out).write_text(json.dumps(result,indent=2)+'\n')
 except Exception as error:
  allowed={'controlled_media_request_invalid','controlled_media_run_invalid','controlled_media_repository_invalid','controlled_media_metadata_bound','controlled_media_foreign_source','controlled_media_untrusted_event','controlled_media_wrong_workflow','controlled_media_source_invalid','controlled_media_job_not_verified','controlled_media_artifact_unverified','controlled_media_source_encoding','controlled_media_recipe_changed','controlled_media_recipe_missing'}
  reason=str(error) if str(error) in allowed else 'controlled_media_provenance_unavailable'
  print(json.dumps({'provenance_verified':False,'reason':reason}));raise SystemExit(1)
 print(json.dumps({'provenance_verified':True,'provider_run':result['provider_run']}))
