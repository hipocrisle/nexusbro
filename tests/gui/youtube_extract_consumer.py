#!/usr/bin/env python3
"""Opt-in Linux consumer extraction test; never launch before host identity approval.

No GPU/browser/cookie/profile access is requested. This is NOT filesystem or
network sandbox acceptance: production proxy plus sampled owned /proc sockets
are reported explicitly. No helpers are installed/downloaded by this wrapper.
"""
import argparse
import collections
import hashlib
import ipaddress
import json
import os
import pathlib
import pwd
import re
import secrets
import select
import signal
import socket
import socketserver
import stat
import subprocess
import sys
import tempfile
import threading
import time
import unittest
import urllib.parse

DOMAINS=('youtube.com','youtube-nocookie.com','googlevideo.com','ytimg.com','ggpht.com','googleapis.com')
LIMIT=65536


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

def safe_summary(raw):
    require(0<len(raw)<=LIMIT,'probe_summary_size')
    lines=raw.decode('utf-8').splitlines();require(len(lines)==1,'probe_summary_lines')
    value=json.loads(lines[0]);require(isinstance(value,dict),'probe_summary_shape')
    require(set(value)<= {'stage','ok','reason','formats','vp9','opus','rejected_formats','diagnostic','impersonate_chrome','chrome_available'},'probe_summary_unknown_fields')
    require(value.get('stage') in ('components','extractor','parser','parsed') and type(value.get('ok')) is bool,'probe_summary_state')
    result={'stage':value['stage'],'ok':value['ok']}
    if 'impersonate_chrome' in value or 'chrome_available' in value:
        require(type(value.get('impersonate_chrome')) is bool and (value.get('chrome_available') is None or type(value['chrome_available']) is bool),'probe_summary_experiment')
        result['impersonate_chrome']=value['impersonate_chrome'];result['chrome_available']=value.get('chrome_available')

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


def private_directory(path, uid):
    info=path.lstat()
    require(stat.S_ISDIR(info.st_mode) and info.st_uid==uid and info.st_mode&0o077==0,'private_directory_refused')


def owned_file(path, uid, limit, *, marker=False):
    info=path.lstat()
    require(stat.S_ISREG(info.st_mode) and info.st_uid==uid and 0<info.st_size<=limit
            and info.st_mode&0o6022==0 and (not marker or info.st_mode&0o777==0o600),'owned_file_refused')
    return info


def digest(path,uid,limit):
    owned_file(path,uid,limit)
    descriptor=os.open(path,os.O_RDONLY|os.O_NOFOLLOW)
    value=hashlib.sha256();count=0
    with os.fdopen(descriptor,'rb') as stream:
        actual=os.fstat(stream.fileno());expected=path.lstat()
        require((actual.st_dev,actual.st_ino)==(expected.st_dev,expected.st_ino),'owned_file_changed')
        for chunk in iter(lambda:stream.read(65536),b''):
            count+=len(chunk);require(count<=limit,'owned_file_oversized');value.update(chunk)
    return value.hexdigest()


def marker_valid(value,uid,video,now):
    require(isinstance(value,dict) and set(value)=={'schema','purpose','uid','video','expires_unix'},'marker_refused')
    require(type(uid) is int and uid>0 and type(value['uid']) is int and value['uid']==uid,'marker_uid_refused')
    require(value['schema']==1 and type(value['schema']) is int
            and value['purpose']=='youtube-anonymous-consumer-probe' and value['video']==video,'marker_refused')
    require(type(value['expires_unix']) is int and now<value['expires_unix']<=now+600,'marker_expired')


def kernel_row(pid):
    try:
        text=pathlib.Path('/proc',str(pid),'stat').read_text();tail=text[text.rindex(')')+2:].split()
        return {'pid':pid,'group':int(tail[2]),'session':int(tail[3]),'birth':tail[19],'zombie':tail[0]=='Z'}
    except (FileNotFoundError,ProcessLookupError):return None


def group_members(group):
    result=[]
    for entry in pathlib.Path('/proc').iterdir():
        if entry.name.isdigit():
            try:
                if entry.stat().st_uid!=os.getuid():continue
            except FileNotFoundError:continue
            row=kernel_row(int(entry.name))
            if row and row['group']==group and row['session']==group and not row['zombie']:result.append(row)
    return result


def proc_sockets():
    result={}
    for name,version in (('tcp',4),('tcp6',6),('udp',4),('udp6',6)):
        for line in pathlib.Path('/proc/net',name).read_text().splitlines()[1:]:
            fields=line.split();remote,port=fields[2].split(':');local,lport=fields[1].split(':')
            def address(raw):
                data=bytes.fromhex(raw)
                if version==4:data=data[::-1]
                else:data=b''.join(data[i:i+4][::-1] for i in range(0,16,4))
                ip=ipaddress.ip_address(data)
                return ip.ipv4_mapped if version==6 and ip.ipv4_mapped else ip
            remote_ip=address(remote);local_ip=address(local)
            denied=(int(port,16)!=0 and not remote_ip.is_loopback)
            if name.startswith('udp') and not remote_ip.is_loopback and not local_ip.is_loopback:denied=True
            if fields[3]=='0A' and not local_ip.is_loopback:denied=True
            result[fields[9]]=denied
    for line in pathlib.Path('/proc/net/unix').read_text().splitlines()[1:]:
        fields=line.split()
        if len(fields)>=7:result[fields[6]]=False
    for line in pathlib.Path('/proc/net/netlink').read_text().splitlines()[1:]:
        fields=line.split()
        if fields:result[fields[-1]]=False
    return result


def inspect_sockets(row,table):
    before=kernel_row(row['pid'])
    if not before or before['birth']!=row['birth'] or before['zombie']:return None
    try:
        for item in pathlib.Path('/proc',str(row['pid']),'fd').iterdir():
            try:link=os.readlink(item)
            except FileNotFoundError:continue
            match=re.fullmatch(r'socket:\[(\d+)\]',link)
            if match:
                inode=match[1]
                if inode not in table:table.update(proc_sockets())
                if inode not in table:
                    try:
                        if os.readlink(item)!=link:continue
                    except FileNotFoundError:continue
                    raise AssertionError('socket_observation_unknown')
                if table[inode]:return False
    except (FileNotFoundError,ProcessLookupError):return None
    except PermissionError:
        current=kernel_row(row['pid'])
        if not current or current['birth']!=row['birth'] or current['zombie']:return None
        raise AssertionError('socket_observation_denied')
    after=kernel_row(row['pid'])
    return True if after and after['birth']==row['birth'] and not after['zombie'] else None


ADMISSION_CODES={'consumer_feature_or_platform_unavailable','consumer_build_seal_missing','consumer_mode_refused',
 'consumer_uid_refused','consumer_marker_refused','consumer_marker_expired','consumer_directory_refused',
 'consumer_file_refused','consumer_home_unavailable','consumer_executable_refused','consumer_video_refused',
 'consumer_route_refused','consumer_manifest_refused','consumer_component_refused','consumer_marker_consumed','consumer_clock_invalid'}


def summary(raw):
    require(0<len(raw)<=LIMIT,'summary_size')
    lines=raw.decode('utf-8').splitlines();require(len(lines)==1,'summary_lines')
    value=json.loads(lines[0])
    if isinstance(value,dict) and value.get('stage')=='admission':
        require(set(value)=={'stage','ok','reason'} and value['ok'] is False
                and value['reason'] in ADMISSION_CODES,'admission_summary_refused')
        return value
    return safe_summary(raw)


def run(args):
    require(sys.platform=='linux' and os.environ.get('GITHUB_ACTIONS')!='true','local_linux_only')
    uid=os.getuid();require(uid>0 and uid==os.geteuid()==args.expect_uid and args.allow_local_extractor,'local_uid_or_consent_refused')
    root=pathlib.Path(args.private_root).absolute();private_directory(root,uid);root=root.resolve(strict=True)
    ai=pathlib.Path(pwd.getpwuid(uid).pw_dir,'AI').resolve(strict=True)
    require(root!=ai and root.is_relative_to(ai),'private_ai_scope_refused')
    executable=pathlib.Path(args.probe_exe).absolute();components=pathlib.Path(args.components).absolute()
    output=pathlib.Path(args.out).absolute()
    require(executable.resolve(strict=True).is_relative_to(root) and components.resolve(strict=True).is_relative_to(root)
            and output.parent.resolve(strict=True)==root and not output.exists(),'private_ai_scope_refused')
    output=output.parent.resolve(strict=True)/output.name
    require(re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._-]{0,95}\.json',output.name) is not None,'output_name_refused')
    require(re.fullmatch(r'[A-Za-z0-9_-]{11}',args.video) is not None,'video_id_refused')
    require(re.fullmatch(r'[0-9a-f]{64}',args.probe_sha256) is not None,'probe_pin_refused')
    require(digest(executable,uid,256*1024*1024)==args.probe_sha256,'probe_pin_mismatch')
    private_directory(components,uid)
    manifest=components/'components.json';manifest_hash=digest(manifest,uid,131072)
    metadata=json.loads(manifest.read_bytes());require(metadata.get('schema')==1 and metadata.get('platform')=='linux-x64','manifest_refused')
    hashes={}
    for name in ('yt-dlp','deno'):
        record=metadata['components'][name];require(record.get('filename')==name,'component_refused')
        actual=digest(components/name,uid,256*1024*1024)
        require(actual==record.get('sha256') and (components/name).stat().st_mode&0o100,'component_refused');hashes[name]=actual
    # No cache/profile directory is borrowed from the owner. Production still
    # controls helper environment; this is not a filesystem sandbox assertion.
    temporary=root/('tmp-'+secrets.token_hex(8));temporary.mkdir(mode=0o700)
    marker=root/('.consumer-admission-'+secrets.token_hex(16)+'.json')
    admission={'schema':1,'purpose':'youtube-anonymous-consumer-probe','uid':uid,'video':args.video,'expires_unix':int(time.time())+300}
    marker_valid(admission,uid,args.video,int(time.time()))
    descriptor=os.open(marker,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
    with os.fdopen(descriptor,'w') as stream:json.dump(admission,stream);stream.flush();os.fsync(stream.fileno())
    report={'schema':1,'completed':False,'playback_tested':False,'scope':'single_anonymous_production_extraction',
            'components_sha256':hashes,'manifest_sha256':manifest_hash,'probe_sha256':args.probe_sha256,
            'caller_uid_matches':True,'filesystem_sandbox':False,'egress_coverage':'production_proxy_plus_sampled_owned_proc_sockets_not_firewall'}
    proxy=None;process=None;threads=[];captured=bytearray();pipe_failed=threading.Event();code=1
    started=time.monotonic();seen=set();samples=0;transient=0;failure=None
    try:
        proxy=ConnectProxy()
        environment={'TMPDIR':str(temporary),'TEMP':str(temporary),'TMP':str(temporary)}
        process=subprocess.Popen([str(executable),'--consumer-marker',str(marker),str(components),proxy.url,args.video],
                  cwd=root,env=environment,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,start_new_session=True)
        identity=kernel_row(process.pid);require(identity and identity['group']==process.pid and identity['session']==process.pid,'owned_group_refused')
        seen.add((identity['pid'],identity['birth']))
        os.setpriority(os.PRIO_PROCESS,process.pid,10)
        def reader(pipe,save):
            count=0
            try:
                while True:
                    chunk=pipe.read(4096)
                    if not chunk:break
                    count+=len(chunk)
                    if count>LIMIT:pipe_failed.set();break
                    if save:captured.extend(chunk)
            except Exception:pipe_failed.set()
            finally:pipe.close()
        for pipe,save in ((process.stdout,True),(process.stderr,False)):
            thread=threading.Thread(target=reader,args=(pipe,save),daemon=True);thread.start();threads.append(thread)
        process.stdin.write(b'GO\n');process.stdin.flush();process.stdin.close()
        deadline=time.monotonic()+70
        while process.poll() is None:
            require(time.monotonic()<deadline,'probe_timeout');require(not pipe_failed.is_set(),'probe_pipe_bound')
            members=group_members(process.pid);require(len(members)<=16,'owned_process_limit')
            table=proc_sockets()
            for row in members:
                seen.add((row['pid'],row['birth']));value=inspect_sockets(row,table)
                if value is None:transient+=1
                else:require(value,'child_direct_connection')
            samples+=1;time.sleep(.05)
        process.wait(timeout=1)
        deadline=time.monotonic()+5
        while group_members(process.pid) and time.monotonic()<deadline:time.sleep(.05)
        require(not group_members(process.pid),'owned_group_survived')
        for thread in threads:thread.join(timeout=1)
        require(all(not thread.is_alive() for thread in threads) and not pipe_failed.is_set(),'probe_pipe_bound')
        value=summary(bytes(captured));report['probe']=value
        report['admission_consumed']=not marker.exists()
        report['processes']={'observed':len(seen),'samples':samples,'transient_exits':transient,'tree_exit_verified':True}
        require(process.returncode==0 and value.get('ok') is True and value.get('stage')=='parsed'
                and report['admission_consumed'],'production_extraction_failed')
        report['completed']=True;code=0
    except Exception as error:
        failure=error
        allowed={'probe_timeout','probe_pipe_bound','owned_process_limit','child_direct_connection','socket_observation_denied',
                 'owned_group_refused','owned_group_survived','socket_observation_unknown','production_extraction_failed','summary_size','summary_lines','admission_summary_refused'}
        report['failure_class']=type(error).__name__;report['failure_code']=str(error) if type(error) is AssertionError and str(error) in allowed else 'observation_or_validation_failed'
    finally:
        if process:
            # Never signal another user's reused numeric process group. A live
            # unreaped direct child, or an exact observed member birth, proves
            # this is still our session. Unknown ownership fails acceptance.
            direct_alive=process.poll() is None
            try:members=group_members(process.pid)
            except Exception:members=[];report['cleanup_unconfirmed']=True
            known_alive=any((row['pid'],row['birth']) in seen for row in members)
            if direct_alive or known_alive:
                try:os.killpg(process.pid,signal.SIGKILL)
                except ProcessLookupError:pass
                try:process.wait(timeout=5)
                except subprocess.TimeoutExpired:report['cleanup_unconfirmed']=True
            elif members:report['cleanup_unconfirmed']=True
            try:
                if group_members(process.pid):report['cleanup_unconfirmed']=True
            except Exception:report['cleanup_unconfirmed']=True
            if report.get('cleanup_unconfirmed'):report['completed']=False;code=1
            report['probe_exit_code']=process.returncode
            for thread in threads:thread.join(timeout=1)
            if captured and 'probe' not in report:
                try:report['probe']=summary(bytes(captured))
                except Exception:report['summary_unavailable']=True
        if proxy:
            report['proxy']=proxy.summary()
            try:proxy.close()
            except Exception:report['cleanup_unconfirmed']=True;report['completed']=False;code=1
        marker.unlink(missing_ok=True)
        # Leave only owned private temporary files for explicit review/cleanup;
        # never delete any pre-existing owner directory or data.
        report['wall_seconds']=round(time.monotonic()-started,3)
        descriptor=os.open(output,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
        with os.fdopen(descriptor,'w') as stream:json.dump(report,stream,indent=2);stream.write('\n')
    print(json.dumps({'completed':report['completed'],'playback_tested':False,'failure_code':report.get('failure_code')}))
    return code


class AdmissionTests(unittest.TestCase):
    def test_marker_uid_expiry_unknown_fields(self):
        value={'schema':1,'purpose':'youtube-anonymous-consumer-probe','uid':1001,'video':'aqz-KE-bpKQ','expires_unix':1200}
        marker_valid(value,1001,'aqz-KE-bpKQ',1000)
        for uid,video,now in ((0,'aqz-KE-bpKQ',1000),(1002,'aqz-KE-bpKQ',1000),(1001,'jNQXAC9IVRw',1000),(1001,'aqz-KE-bpKQ',1200),(1001,'aqz-KE-bpKQ',500)):
            with self.assertRaises(AssertionError):marker_valid(value,uid,video,now)
        with self.assertRaises(AssertionError):marker_valid({**value,'path':'not-admitted'},1001,'aqz-KE-bpKQ',1000)
    def test_symlink_marker_rejected_before_target_read(self):
        with tempfile.TemporaryDirectory() as directory:
            root=pathlib.Path(directory);target=root/'target';target.write_text('not a marker');target.chmod(0o600)
            link=root/'link';link.symlink_to(target)
            with self.assertRaises(AssertionError):owned_file(link,os.getuid(),4096,marker=True)
    def test_proxy_boundaries_and_safe_admission_summary(self):
        self.assertEqual(target('www.youtube.com:443')[1],'youtube.com')
        for value in ('youtube.com.evil.test:443','localhost:443','youtube.com:80','user@youtube.com:443'):
            with self.assertRaises(AssertionError):target(value)
        self.assertFalse(public_address('127.0.0.1'));self.assertFalse(public_address('10.0.0.1'))
        self.assertEqual(summary(b'{"stage":"admission","ok":false,"reason":"consumer_build_seal_missing"}')['reason'],'consumer_build_seal_missing')
        with self.assertRaises(AssertionError):summary(b'{"stage":"admission","ok":false,"reason":"secret"}')


if __name__=='__main__':
    if '--self-test' in sys.argv:
        unittest.main(argv=[sys.argv[0]],exit=True)
    parser=argparse.ArgumentParser(description=__doc__)
    for key in ('probe-exe','components','private-root','out','probe-sha256'):parser.add_argument('--'+key,required=True)
    parser.add_argument('--expect-uid',required=True,type=int);parser.add_argument('--video',default='aqz-KE-bpKQ')
    parser.add_argument('--allow-local-extractor',action='store_true')
    arguments=parser.parse_args()
    try:sys.exit(run(arguments))
    except Exception as error:
        print(json.dumps({'completed':False,'failure_class':type(error).__name__,'failure_code':'local_admission_or_package_refused'}));sys.exit(2)
