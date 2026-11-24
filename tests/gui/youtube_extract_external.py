#!/usr/bin/env python3
"""Real anonymous production extractor probe on isolated GitHub runners only.

This does NOT test video playback or claim complete network sandboxing. Raw
extractor JSON, media URLs, headers and stderr never enter artifacts.
--self-test performs only small pure validation checks, without components.
"""
import argparse
import collections
import hashlib
import ipaddress
import json
import os
import pathlib
import queue
import re
import select
import signal
import socket
import socketserver
import stat
import subprocess
import sys
import threading
import time
import urllib.parse

import psutil

DOMAINS = ('youtube.com','youtube-nocookie.com','googlevideo.com','ytimg.com','ggpht.com','googleapis.com')
LIMIT = 64 * 1024


def require(value, code):
    if not value: raise AssertionError(code)


def target(authority):
    require(isinstance(authority,str) and len(authority)<=260 and authority.isascii()
            and not any(ord(c)<=32 or ord(c)==127 for c in authority), 'invalid_proxy_authority')
    parsed=urllib.parse.urlsplit('//'+authority)
    require(not parsed.username and not parsed.password and parsed.path=='' and not parsed.query and not parsed.fragment,
            'invalid_proxy_authority')
    require(parsed.port==443 and parsed.hostname is not None,'proxy_port_denied')
    host=parsed.hostname.lower()
    require(re.fullmatch(r'[a-z0-9](?:[a-z0-9.-]{0,251}[a-z0-9])?',host) is not None
            and all(label and len(label)<=63 and not label.startswith('-') and not label.endswith('-') for label in host.split('.')),
            'invalid_proxy_hostname')
    domain=next((d for d in DOMAINS if host==d or host.endswith('.'+d)),None)
    require(domain is not None,'proxy_domain_denied')
    return host,domain


def public_address(address):
    value=ipaddress.ip_address(address)
    return value.is_global and not value.is_multicast and not value.is_unspecified and not value.is_reserved


class ConnectProxy:
    def __init__(self):
        self.lock=threading.Lock();self.sockets=set();self.counts=collections.Counter();self.domains=collections.Counter()
        self.slots=threading.BoundedSemaphore(16);owner=self
        class Server(socketserver.ThreadingTCPServer):
            allow_reuse_address=True;daemon_threads=True;block_on_close=False
            def handle_error(self,*_):pass
        class Handler(socketserver.StreamRequestHandler):
            def handle(self):
                upstream=None
                if not owner.slots.acquire(blocking=False):
                    with owner.lock:owner.counts['concurrency_denied']+=1
                    return
                try:
                    self.connection.settimeout(5)
                    with owner.lock:owner.sockets.add(self.connection)
                    line=self.rfile.readline(4097)
                    require(len(line)<=4096 and line.endswith(b'\r\n'),'proxy_request_line_limit')
                    parts=line.decode('ascii').strip().split(' ')
                    require(len(parts)==3 and parts[0]=='CONNECT' and parts[2] in ('HTTP/1.0','HTTP/1.1'),'proxy_method_denied')
                    host,domain=target(parts[1])
                    total=0
                    while True:
                        header=self.rfile.readline(8193);total+=len(header)
                        require(header and total<=16384 and len(header)<=8192,'proxy_header_limit')
                        if header==b'\r\n':break
                    with owner.lock:
                        owner.counts['connect_attempts']+=1;owner.domains[domain]+=1
                    addresses=socket.getaddrinfo(host,443,type=socket.SOCK_STREAM)
                    require(addresses and all(public_address(a[4][0]) for a in addresses),'proxy_private_dns_denied')
                    for family,kind,protocol,_,address in addresses:
                        candidate=socket.socket(family,kind,protocol);candidate.settimeout(5)
                        try:candidate.connect(address);upstream=candidate;break
                        except OSError:candidate.close()
                    require(upstream is not None,'proxy_upstream_unavailable')
                    with owner.lock:owner.sockets.add(upstream);owner.counts['connections_established']+=1
                    self.wfile.write(b'HTTP/1.1 200 Connection Established\r\n\r\n');self.wfile.flush()
                    # Each CONNECT lasts at most the wrapper's extraction window.
                    deadline=time.monotonic()+70
                    while time.monotonic()<deadline:
                        readable,_,_=select.select([self.connection,upstream],[],[],.5)
                        for source in readable:
                            data=source.recv(65536)
                            if not data:return
                            destination=upstream if source is self.connection else self.connection
                            destination.sendall(data)
                    with owner.lock:owner.counts['tunnel_deadline']+=1
                except Exception as error:
                    with owner.lock:
                        owner.counts['requests_refused_or_failed']+=1
                        if isinstance(error,AssertionError) and str(error) in {'proxy_domain_denied','proxy_port_denied','proxy_private_dns_denied'}:
                            owner.counts[str(error)]+=1
                    try:self.wfile.write(b'HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\nContent-Length: 0\r\n\r\n');self.wfile.flush()
                    except OSError:pass
                finally:
                    for connection in (upstream,self.connection):
                        if connection:
                            with owner.lock:owner.sockets.discard(connection)
                            try:connection.shutdown(socket.SHUT_RDWR)
                            except OSError:pass
                            connection.close()
                    owner.slots.release()
        self.server=Server(('127.0.0.1',0),Handler)
        self.thread=threading.Thread(target=self.server.serve_forever,daemon=True);self.thread.start()
        self.url='http://127.0.0.1:'+str(self.server.server_address[1])
    def summary(self):
        with self.lock:return {'connections':dict(self.counts),'domains':dict(self.domains)}
    def close(self):
        self.server.shutdown();self.server.server_close()
        with self.lock:connections=list(self.sockets)
        for connection in connections:
            try:connection.shutdown(socket.SHUT_RDWR)
            except OSError:pass
            connection.close()
        self.thread.join(timeout=5)


def digest(path,maximum):
    require(not path.is_symlink() and path.is_file(),'component_file_type')
    count=0;hash_=hashlib.sha256()
    with path.open('rb') as stream:
        require(stat.S_ISREG(os.fstat(stream.fileno()).st_mode),'component_file_type')
        for block in iter(lambda:stream.read(65536),b''):
            count+=len(block);require(count<=maximum,'component_file_limit');hash_.update(block)
    require(count>0,'component_file_empty')
    return hash_.hexdigest()


def verify_components(directory):
    require(directory.is_dir() and not directory.is_symlink(),'component_directory')
    manifest=directory/'components.json'
    require(manifest.is_file() and not manifest.is_symlink() and manifest.stat().st_size<=128*1024,'component_manifest')
    metadata=json.loads(manifest.read_text())
    source=pathlib.Path(__file__).resolve().parents[2]/'docs'/'YOUTUBE-COMPONENTS.json'
    pinned=json.loads(source.read_text());platform='windows-x64' if sys.platform=='win32' else 'linux-x64'
    require(metadata.get('schema')==1 and metadata.get('platform')==platform and pinned.get('schema')==1,'component_manifest_schema')
    require(metadata.get('source_manifest_sha256')==digest(source,128*1024),'component_source_manifest_mismatch')
    result={}
    for name in ('yt-dlp','deno'):
        pin=pinned['platforms'][platform][name];record=metadata['components'][name]
        require(record['filename']==pin['filename'] and record['asset_sha256']==pin['sha256'] and record['version']==pin['version'],'component_pin_mismatch')
        actual=digest(directory/pin['filename'],256*1024*1024)
        require(actual==record['sha256'] and re.fullmatch('[0-9a-f]{64}',actual),'component_digest_mismatch')
        if not pin['archive']:require(actual==pin['sha256'],'component_asset_digest_mismatch')
        result[name]={'sha256':actual,'version':record['version']}
    return result


def safe_summary(raw):
    require(0<len(raw)<=LIMIT,'probe_summary_size')
    lines=raw.decode('utf-8').splitlines();require(len(lines)==1,'probe_summary_lines')
    value=json.loads(lines[0]);require(isinstance(value,dict),'probe_summary_shape')
    require(set(value)<= {'stage','ok','reason','formats','vp9','opus','rejected_formats','diagnostic'},'probe_summary_unknown_fields')
    require(value.get('stage') in ('components','extractor','parser','parsed') and type(value.get('ok')) is bool,'probe_summary_state')
    result={'stage':value['stage'],'ok':value['ok']}
    if 'reason' in value:
        reasons={'InvalidComponents','InvalidRoute','Cancelled','SpawnFailed','OutputLimit','ReadFailed','WaitFailed','ProcessFailed','TimedOut','ReapFailed','EmptyOutput',
                 'UnsupportedInvocation','HttpForbidden','RateLimited','TlsFailure','ChallengeRequired','VideoUnavailable','HttpBadRequest','HttpNotFound','HttpServerError','NetworkFailure','RuntimeFailure','ChallengeSolverUnavailable','FormatUnavailable','FfmpegUnavailable','ExtractionFailure',
                 'oversized','invalid_json','wrong_video','unsupported_video','invalid_metadata','invalid_format','unsafe_url','unsafe_headers','no_supported_formats'}
        result['reason']=value['reason'] if isinstance(value['reason'],str) and value['reason'] in reasons else 'unclassified_native_reason'
    if 'diagnostic' in value:
        diagnostic=value['diagnostic']
        flags={'usage','http_400','http_403','http_404','http_429','http_5xx','tls','network','runtime','solver','format_unavailable','ffmpeg_missing','extractor','error_prefix','warning_prefix','file_missing','permission_denied','invalid_argument','home_unavailable','connection_reset','connection_closed','timeout','dns'}
        require(isinstance(diagnostic,dict) and set(diagnostic)==flags|{'exit_code','stderr_bytes','errno','win_error'},'probe_summary_shape')
        require(all(type(diagnostic[k]) is bool for k in flags),'probe_summary_shape')
        require(type(diagnostic['stderr_bytes']) is int and 0<=diagnostic['stderr_bytes']<=65536,'probe_summary_counter')
        require(diagnostic['exit_code'] is None or (type(diagnostic['exit_code']) is int and -(1<<31)<=diagnostic['exit_code']<(1<<31)),'probe_summary_counter')
        for field in ('errno','win_error'):
            require(diagnostic[field] is None or (type(diagnostic[field]) is int and 0<=diagnostic[field]<=65535),'probe_summary_counter')
        result['diagnostic']={k:diagnostic[k] for k in sorted(flags|{'exit_code','stderr_bytes','errno','win_error'})}
    for key in ('formats','vp9','opus','rejected_formats'):
        if key in value:
            require(type(value[key]) is int and 0<=value[key]<=256,'probe_summary_counter')
            result[key]=value[key]
    if result['ok']:
        require(result['stage']=='parsed' and result.get('vp9',0)>0 and result.get('opus',0)>0 and result.get('formats',0)>0,'probe_summary_false_ready')
    return result


class OwnedTree:
    """GO barrier ensures the Windows Job exists before any helper spawns."""
    def __init__(self,process):
        self.pid=process.pid;self.job=None
        if sys.platform=='win32':
            import ctypes
            from ctypes import wintypes
            self.ctypes=ctypes;self.kernel=ctypes.WinDLL('kernel32',use_last_error=True)
            class Basic(ctypes.Structure):
                _fields_=[('process_time',ctypes.c_longlong),('job_time',ctypes.c_longlong),('flags',wintypes.DWORD),
                    ('min_ws',ctypes.c_size_t),('max_ws',ctypes.c_size_t),('active_limit',wintypes.DWORD),
                    ('affinity',ctypes.c_size_t),('priority',wintypes.DWORD),('scheduling',wintypes.DWORD)]
            class IO(ctypes.Structure):_fields_=[('counter'+str(i),ctypes.c_ulonglong) for i in range(6)]
            class Limits(ctypes.Structure):
                _fields_=[('basic',Basic),('io',IO),('process_memory',ctypes.c_size_t),('job_memory',ctypes.c_size_t),
                    ('peak_process',ctypes.c_size_t),('peak_job',ctypes.c_size_t)]
            k=self.kernel;k.CreateJobObjectW.argtypes=[ctypes.c_void_p,wintypes.LPCWSTR];k.CreateJobObjectW.restype=wintypes.HANDLE
            k.SetInformationJobObject.argtypes=[wintypes.HANDLE,ctypes.c_int,ctypes.c_void_p,wintypes.DWORD];k.SetInformationJobObject.restype=wintypes.BOOL
            k.AssignProcessToJobObject.argtypes=[wintypes.HANDLE,wintypes.HANDLE];k.AssignProcessToJobObject.restype=wintypes.BOOL
            k.QueryInformationJobObject.argtypes=[wintypes.HANDLE,ctypes.c_int,ctypes.c_void_p,wintypes.DWORD,ctypes.c_void_p];k.QueryInformationJobObject.restype=wintypes.BOOL
            k.TerminateJobObject.argtypes=[wintypes.HANDLE,wintypes.UINT];k.TerminateJobObject.restype=wintypes.BOOL
            k.CloseHandle.argtypes=[wintypes.HANDLE]
            self.job=k.CreateJobObjectW(None,None);require(bool(self.job),'probe_job_create_failed')
            limits=Limits();limits.basic.flags=0x2000
            try:
                require(bool(k.SetInformationJobObject(self.job,9,ctypes.byref(limits),ctypes.sizeof(limits))),'probe_job_limit_failed')
                require(bool(k.AssignProcessToJobObject(self.job,int(process._handle))),'probe_job_assign_failed')
            except Exception:self.close();raise
    def members(self):
        if self.job:
            c=self.ctypes;buffer=c.create_string_buffer(8+c.sizeof(c.c_size_t)*128)
            require(bool(self.kernel.QueryInformationJobObject(self.job,3,buffer,len(buffer),None)),'probe_job_members_failed')
            assigned=c.c_uint32.from_buffer(buffer,0).value;count=c.c_uint32.from_buffer(buffer,4).value
            require(assigned<=128 and count==assigned,'probe_job_member_limit')
            ids=list((c.c_size_t*count).from_buffer(buffer,8))
        else:
            ids=[]
            for p in psutil.process_iter(['pid']):
                try:
                    if os.getpgid(p.pid)==self.pid:ids.append(p.pid)
                except (ProcessLookupError,PermissionError):pass
        result=[]
        for pid in ids:
            try:
                p=psutil.Process(pid)
                if p.status()!=psutil.STATUS_ZOMBIE:result.append(p)
            except psutil.NoSuchProcess:pass
        return result
    def kill(self):
        if self.job:require(bool(self.kernel.TerminateJobObject(self.job,1)),'probe_job_terminate_failed')
        else:
            try:os.killpg(self.pid,signal.SIGKILL)
            except ProcessLookupError:pass
    def close(self):
        if self.job:self.kernel.CloseHandle(self.job);self.job=None


def run_probe(executable,components,proxy,video,diagnostics):
    kwargs={'creationflags':subprocess.CREATE_NEW_PROCESS_GROUP} if sys.platform=='win32' else {'start_new_session':True}
    env={k:os.environ[k] for k in ('SystemRoot','WINDIR','TEMP','TMP','TMPDIR') if k in os.environ}
    env['GITHUB_ACTIONS']='true'
    # Explicit CI-only public recipient/output; never inherit arbitrary env.
    for name in ('NEXUSBRO_TEST_DIAGNOSTIC_PUBLIC_KEY','NEXUSBRO_TEST_DIAGNOSTIC_OUTPUT'):
        value=os.environ.get(name,'')
        require(len(value)<=8192,'diagnostic_input_limit')
        if value:env[name]=value
    process=subprocess.Popen([str(executable),str(components),proxy.url,video],stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=env,**kwargs)
    root=psutil.Process(process.pid);owned={(root.pid,root.create_time())};streams={'stdout':bytearray()};failed=threading.Event();reader_error=[]
    def reader(pipe,name):
        count=0
        try:
            while True:
                block=pipe.read(4096)
                if not block:break
                count+=len(block)
                if count>LIMIT:reader_error.append('pipe_limit');failed.set();break
                if name=='stdout':streams[name].extend(block)
        except Exception:reader_error.append('pipe_read');failed.set()
        finally:pipe.close()
    workers=[threading.Thread(target=reader,args=(process.stdout,'stdout'),daemon=True),threading.Thread(target=reader,args=(process.stderr,'stderr'),daemon=True)]
    for worker in workers:worker.start()
    deadline=time.monotonic()+70;external=False;network_complete=True;failure=None;network_samples=0;tree=None
    transient_exits=0;access_denied=collections.Counter()
    try:
        tree=OwnedTree(process)
        process.stdin.write(b'GO\n');process.stdin.flush();process.stdin.close()
        while process.poll() is None:
            try:
                members=tree.members()
                for member in members:
                    try:
                        owned.add((member.pid,member.create_time()))
                        for connection in member.net_connections(kind='tcp'):
                            if connection.raddr and not ipaddress.ip_address(connection.raddr.ip).is_loopback:external=True
                    except psutil.NoSuchProcess:transient_exits+=1
                    except psutil.AccessDenied:
                        # /proc may disappear between the ownership snapshot
                        # and socket inspection. Only proven exit/zombie/PID
                        # replacement is transient; a live denied PID is unknown.
                        try:
                            ended=(not member.is_running() or member.status()==psutil.STATUS_ZOMBIE
                                   or (member.pid,member.create_time()) not in owned)
                        except psutil.NoSuchProcess:ended=True
                        except psutil.AccessDenied:ended=False
                        if ended:transient_exits+=1
                        else:
                            network_complete=False
                            access_denied['probe' if member.pid==process.pid else 'helper']+=1
                network_samples+=1
            except psutil.NoSuchProcess:pass
            if external:failure='observed_child_direct_egress';break
            if failed.is_set():failure='probe_pipe_limit_or_read';break
            if time.monotonic()>=deadline:failure='probe_timeout';break
            time.sleep(.05)
    finally:
        # Kill/verify the whole observed tree on every incomplete/failed run;
        # never kill only the root and leave an extractor/runtime behind.
        if process.poll() is None:
            if tree:
                try:tree.kill()
                except Exception:failure='probe_tree_terminate_failed'
            try:owned.update((p.pid,p.create_time()) for p in root.children(recursive=True))
            except psutil.NoSuchProcess:pass
            if sys.platform!='win32':
                try:os.killpg(process.pid,signal.SIGKILL)
                except ProcessLookupError:pass
            for pid,created in sorted(owned,reverse=True):
                try:
                    p=psutil.Process(pid)
                    if p.create_time()==created:p.kill()
                except psutil.NoSuchProcess:pass
        try:process.wait(timeout=5)
        except subprocess.TimeoutExpired:failure='probe_reap_timeout'
        until=time.monotonic()+5
        alive=[]
        while True:
            alive=[]
            for pid,created in owned:
                try:
                    p=psutil.Process(pid)
                    if p.create_time()==created and p.is_running():alive.append(p)
                except psutil.NoSuchProcess:pass
            if tree:
                try:
                    for p in tree.members():
                        if all(p.pid!=a.pid for a in alive):alive.append(p)
                except Exception:failure='probe_tree_observation_failed'
            if not alive or time.monotonic()>=until:break
            time.sleep(.05)
        if alive:
            failure='probe_descendants_survived'
            if tree:
                try:tree.kill()
                except Exception:pass
            for p in alive:
                try:p.kill()
                except psutil.NoSuchProcess:pass
        for worker in workers:worker.join(timeout=2)
        if any(worker.is_alive() for worker in workers):failure='probe_pipe_not_closed'
        if tree:tree.close()
        if not process.stdin.closed:process.stdin.close()
    evidence={'observed_processes':len(owned),'tree_exit_verified':failure not in ('probe_descendants_survived','probe_reap_timeout'),
        'network_samples':network_samples,'observed_child_direct_connections':int(external),
        'network_observation_complete':network_complete,'transient_exited_process_observations':transient_exits,
        'live_socket_access_denied':dict(access_denied),
        'egress_coverage':'explicit_production_proxy_and_sampled_child_sockets_not_firewall'}
    diagnostics.update(probe_exit_code=process.returncode,processes=evidence)
    # Preserve the sanitized production outcome even if OS observation failed.
    # Otherwise a network permission error hides the actual extractor result.
    summary=None
    try:
        summary=safe_summary(bytes(streams['stdout']))
        diagnostics['probe']=summary
    except Exception as error:diagnostics['probe_summary_failure_class']=type(error).__name__
    require(failure is None,failure or 'probe_failed')
    require(network_complete,'probe_network_observation_incomplete')
    require(not failed.is_set(),'probe_pipe_limit_or_read')
    require(summary is not None,'probe_summary_unavailable')
    return process.returncode,summary,evidence


def self_test():
    for authority in ('youtube.com:443','www.youtube.com:443','r1.googlevideo.com:443'):
        require(target(authority)[1] in DOMAINS,'self_test_allowed_domain')
    for authority in ('youtube.com.evil.test:443','youtube.com@evil.test:443','youtube.com:80','127.0.0.1:443','youtube.com:443/path','evilgooglevideo.com:443'):
        try:target(authority)
        except (AssertionError,ValueError):continue
        raise AssertionError('self_test_denied_domain')
    for address in ('127.0.0.1','169.254.169.254','::1','10.0.0.1','224.0.0.1'):
        require(not public_address(address),'self_test_nonpublic_address')
    good={'stage':'parsed','ok':True,'formats':2,'vp9':1,'opus':1,'rejected_formats':0}
    require(safe_summary(json.dumps(good).encode())==good,'self_test_summary')
    for bad in (dict(good,opus=0),dict(good,formats=True),dict(good,url='private'),dict(good,formats=257)):
        try:safe_summary(json.dumps(bad).encode())
        except AssertionError:continue
        raise AssertionError('self_test_reject_summary')
    print('Pure domain boundary/private-address/sanitized summary checks passed; no components or network executed')


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--probe-exe');parser.add_argument('--components');parser.add_argument('--out')
    parser.add_argument('--video',default='aqz-KE-bpKQ');parser.add_argument('--self-test',action='store_true')
    args=parser.parse_args()
    if args.self_test:self_test();return 0
    require(os.environ.get('GITHUB_ACTIONS')=='true' and sys.platform in ('win32','linux'),'isolated_native_ci_required')
    require(args.probe_exe and args.components and args.out,'probe_arguments_required')
    require(re.fullmatch(r'[A-Za-z0-9_-]{11}',args.video) is not None,'invalid_video_id')
    output=pathlib.Path(args.out).resolve();require(not output.exists(),'output_already_exists');output.parent.mkdir(parents=True,exist_ok=True)
    report={'schema':1,'completed':False,'playback_tested':False,'platform':sys.platform,
        'scope':'real_anonymous_production_extractor_and_parser_only','checks':[]};proxy=None;code=1
    try:
        components=pathlib.Path(args.components).absolute();executable=pathlib.Path(args.probe_exe).absolute()
        require(executable.is_file() and not executable.is_symlink(),'probe_executable_missing')
        report['components']=verify_components(components)
        report['component_verification']='packaged_files_match_manifest_and_pinned_asset_metadata_not_compiled_app_pins'
        report['probe_exe_sha256']=digest(executable,256*1024*1024)
        proxy=ConnectProxy();started=time.monotonic()
        status,summary,processes=run_probe(executable,components,proxy,args.video,report)
        report.update(wall_seconds=round(time.monotonic()-started,3),probe_exit_code=status,probe=summary,processes=processes,proxy=proxy.summary())
        require(status==0 and summary['ok'] and summary['stage']=='parsed','anonymous_extraction_or_supported_formats_failed')
        require(report['proxy']['connections'].get('connections_established',0)>0,'no_actual_proxy_connections')
        require(not report['proxy']['connections'].get('proxy_domain_denied',0),'extractor_requested_unapproved_domain')
        report['checks'].append('actual_anonymous_production_sidecar_and_parser_admit_vp9_and_opus_via_restricted_connect_proxy')
        report['completed']=True;code=0
    except Exception as error:
        allowed={'probe_executable_missing','component_directory','component_manifest','component_manifest_schema','component_source_manifest_mismatch',
            'component_pin_mismatch','component_digest_mismatch','component_asset_digest_mismatch','component_file_type','component_file_limit','component_file_empty',
            'observed_child_direct_egress','probe_pipe_limit_or_read','probe_timeout','probe_reap_timeout','probe_descendants_survived','probe_pipe_not_closed',
            'probe_network_observation_incomplete','probe_tree_terminate_failed','probe_tree_observation_failed','probe_job_create_failed',
            'probe_job_limit_failed','probe_job_assign_failed','probe_job_members_failed','probe_job_member_limit','probe_job_terminate_failed','probe_summary_size','probe_summary_lines','probe_summary_shape','probe_summary_unknown_fields','probe_summary_state',
            'probe_summary_counter','probe_summary_false_ready','probe_summary_unavailable','anonymous_extraction_or_supported_formats_failed','no_actual_proxy_connections','extractor_requested_unapproved_domain'}
        report['failure_class']=type(error).__name__;report['failure_code']=str(error) if isinstance(error,AssertionError) and str(error) in allowed else 'unclassified'
        if proxy:report['proxy']=proxy.summary()
    finally:
        if proxy:proxy.close()
        output.write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps({'completed':report['completed'],'playback_tested':False}))
    return code


if __name__=='__main__':sys.exit(main())
