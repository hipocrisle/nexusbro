#!/usr/bin/env python3
"""GHA-only real CEF controlled dual-WebM playback, NOT YouTube extraction.

Only packaged native commands admit the fixed fixture document. Playback actions
are physical mouse clicks. Public output contains numeric receipts, never URLs,
headers, certificate material, process arguments or raw native errors.
"""
import argparse
import datetime
import http.server
import json
import os
import pathlib
import select
import secrets
import shutil
import socket
import socketserver
import ssl
import stat
import subprocess
import sys
import tempfile
import threading
import time
import urllib.parse

from security_native import Device, call, js_wait, painted, require, super_record, wait_for
from tab_routes import WindowsDevice, switch_super, rows, click_control
from resource_soak import foreground as bare_foreground, kernel_birth
from youtube_privacy_scan import scan_profile, _open_readonly, _windows_info
from hibernation_cookie_probe import WindowsFixtureTrust, tls_proxy_preflight

HOST = 'fixture.googlevideo.com'
CAPTION_HOST = 'www.youtube.com'


class OwnedWindowManager:
    """Real WM only on this fixture's private Xvfb, never the owner desktop."""
    def __init__(self, device, root):
        require(sys.platform.startswith('linux') and device.x.poll() is None,
                'Owned Xvfb required for native minimize')
        require(bool(shutil.which('openbox')), 'Owned minimize window manager unavailable')
        self.device = device
        self.env = {**os.environ, 'DISPLAY': device.display,
                    'HOME': str(root), 'XDG_CONFIG_HOME': str(root / 'config'),
                    'XDG_CACHE_HOME': str(root / 'cache')}
        root.mkdir(parents=True)
        config = root / 'rc.xml'
        config.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc"><focus><focusNew>yes</focusNew></focus><keyboard><keybind key="A-F4"><action name="Close"/></keybind></keyboard></openbox_config>')
        self.process = subprocess.Popen(['openbox', '--config-file', str(config)], env=self.env,
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                       start_new_session=True)
        def ready():
            require(self.process.poll() is None, 'Owned minimize window manager exited')
            r = subprocess.run(['xprop', '-root', '_NET_SUPPORTING_WM_CHECK'], env=self.env,
                               capture_output=True, text=True, timeout=3)
            return r.returncode == 0 and 'window id #' in r.stdout
        try:
            wait_for(ready, 'Owned minimize window manager not ready', 15)
        except BaseException:
            self.close()
            raise
    def close(self):
        if self.process.poll() is None:
            self.process.terminate()
            self.process.wait(timeout=5)


def native_owner(device):
    import psutil
    client=device.client
    if sys.platform=='win32':
        title=call(client,'plugin:window|title',label='main')
        owned={device.app.pid}|{p.pid for p in psutil.Process(device.app.pid).children(recursive=True)}
        from pywinauto import Desktop
        windows=[w for w in Desktop(backend='uia').windows() if w.process_id() in owned and w.window_text()==title]
        require(len(windows)==1,'Native minimize owner ambiguous')
        return windows[0]
    wm=device.player_wm
    import re
    clock='linux_ticks'
    parent_birth=kernel_birth(device.app.pid,clock)
    record={'observations':0,'zero_observations':0,'max_exact_matches':0,'waited_ms':0,
            'ready':False,'initial':None,'last':None}
    diagnostics=getattr(device,'player_owner_diagnostics',None)
    if diagnostics is None:
        diagnostics={'calls':0,'startup':record,'latest':record}
        device.player_owner_diagnostics=diagnostics
    diagnostics['calls']+=1;diagnostics['latest']=record
    started=time.monotonic();deadline=started+10
    remaining=lambda:max(.01,min(3,deadline-time.monotonic()))
    while time.monotonic()<deadline:
        require(device.app.poll() is None and kernel_birth(device.app.pid,clock)==parent_birth,
                'Native owner parent identity changed')
        title=call(client,'plugin:window|title',label='main',timeout=remaining())
        owned={device.app.pid}|{p.pid for p in psutil.Process(device.app.pid).children(recursive=True)}
        require(len(owned)<=128,'Native owner observation outside bounds')
        listing=subprocess.check_output(['xprop','-root','_NET_CLIENT_LIST'],env=wm.env,text=True,
                                        stderr=subprocess.DEVNULL,timeout=remaining())
        clients=re.findall(r'0x[0-9a-fA-F]+',listing)
        counts={'ewmh_clients':len(clients),'title_matches':0,'owned_pid_matches':0,
                'exe_matches':0,'cef_exclusions':0,'exact_matches':0,'birth_verified':0,'transient_candidates':0,'scan_complete':True}
        require(len(clients)<=128,'Native owner observation outside bounds')
        matches=[]
        for xid in clients:
            if time.monotonic()>=deadline:
                counts['scan_complete']=False;break
            try:
                name=subprocess.check_output(['xdotool','getwindowname',xid],env=wm.env,text=True,
                                             stderr=subprocess.DEVNULL,timeout=remaining()).strip()
                if name!=title:continue
                counts['title_matches']+=1
                pid=int(subprocess.check_output(['xdotool','getwindowpid',xid],env=wm.env,text=True,
                                                stderr=subprocess.DEVNULL,timeout=remaining()).strip())
                if pid not in owned:continue
                counts['owned_pid_matches']+=1
                birth=kernel_birth(pid,clock)
                process=psutil.Process(pid);argv=process.cmdline()
                if '--nb-dvizhok' in argv or any(a.startswith('--type=') for a in argv):
                    counts['cef_exclusions']+=1;continue
                if pathlib.Path(process.exe()).resolve()!=device.exe.resolve():continue
                counts['exe_matches']+=1
                if kernel_birth(pid,clock)!=birth:continue
                counts['birth_verified']+=1
                matches.append(xid)
            except subprocess.TimeoutExpired:
                counts['scan_complete']=False;break
            except (psutil.NoSuchProcess,subprocess.CalledProcessError):
                # A manager may still be adopting or retiring a client. This
                # observation does not grant authority or perform an action.
                counts['transient_candidates']+=1
        counts['exact_matches']=len(matches)
        record['observations']+=1
        record['waited_ms']=round((time.monotonic()-started)*1000)
        record['max_exact_matches']=max(record['max_exact_matches'],len(matches))
        if record['initial'] is None:record['initial']=counts.copy()
        record['last']=counts.copy()
        require(len(matches)<=1,'Native minimize owner ambiguous')
        if len(matches)==1 and counts['scan_complete']:
            record['ready']=True
            return matches[0]
        if not matches:record['zero_observations']+=1
        if not counts['scan_complete']:break
        time.sleep(min(.1,max(0,deadline-time.monotonic())))
    require(False,'Native minimize owner ambiguous')



def valid_native_app_binding(pid,wrapper,family,exe_matches,args,birth_before,birth_after,
                             wrapper_birth_before,wrapper_birth_after):
    """Pure admission contract; only the exact native app may own its receipt."""
    return (type(pid) is int and 0<pid<=0xffffffff and type(wrapper) is int and 0<wrapper<=0xffffffff
            and isinstance(family,set) and 1<=len(family)<=128
            and all(type(member) is int and 0<member<=0xffffffff for member in family)
            and wrapper in family and pid in family
            and exe_matches is True and isinstance(args,list) and 1<=len(args)<=128
            and all(isinstance(arg,str) and len(arg)<=4096 for arg in args)
            and not any(arg in ('--nb-dvizhok','--type') or arg.startswith(('--nb-dvizhok=','--type=')) for arg in args)
            and isinstance(birth_before,str) and birth_before.isdigit() and birth_before==birth_after
            and isinstance(wrapper_birth_before,str) and wrapper_birth_before.isdigit()
            and wrapper_birth_before==wrapper_birth_after)


def native_app_identity(device):
    """Bind verified shell HWND/XID to the actual app, separately from its launcher."""
    import psutil
    require(device.app is not None and device.app.poll() is None,'Native app identity invalid')
    clock='windows_filetime' if sys.platform=='win32' else 'linux_ticks'
    wrapper=device.app.pid;wrapper_birth=kernel_birth(wrapper,clock)
    owner=native_owner(device)
    def window_pid():
        if sys.platform=='win32':return owner.process_id()
        result=subprocess.run(['xdotool','getwindowpid',str(owner)],env=device.player_wm.env,
            capture_output=True,text=True,timeout=3,check=True)
        require(len(result.stdout)<=32 and result.stdout.strip().isdecimal(),'Native app identity invalid')
        return int(result.stdout.strip())
    def family():
        members={wrapper}|{p.pid for p in psutil.Process(wrapper).children(recursive=True)}
        require(len(members)<=128,'Native app identity invalid')
        return members
    pid=window_pid();first_family=family();birth=kernel_birth(pid,clock)
    process=psutil.Process(pid);args=process.cmdline()
    exe_matches=pathlib.Path(process.exe()).resolve()==device.exe.resolve()
    second_family=family();after=kernel_birth(pid,clock);wrapper_after=kernel_birth(wrapper,clock)
    require(valid_native_app_binding(pid,wrapper,first_family,exe_matches,args,birth,after,
                                    wrapper_birth,wrapper_after)
            and pid in second_family and window_pid()==pid and device.app.poll() is None
            and kernel_birth(pid,clock)==birth and kernel_birth(wrapper,clock)==wrapper_birth,
            'Native app identity invalid')
    return dict(pid=pid,birth=birth,wrapper_pid=wrapper,wrapper_birth=wrapper_birth,clock=clock,window=owner)


def native_app_execution_alive(identity):
    import psutil
    try:
        process=psutil.Process(identity['pid'])
        return (process.is_running() and process.status()!=psutil.STATUS_ZOMBIE
                and kernel_birth(identity['pid'],identity['clock'])==identity['birth'])
    except psutil.NoSuchProcess:return False


def native_app_identity_alive(device,identity):
    import psutil
    if device.app is None or device.app.pid!=identity['wrapper_pid'] or device.app.poll() is not None:return False
    try:
        return (kernel_birth(identity['wrapper_pid'],identity['clock'])==identity['wrapper_birth']
                and native_app_execution_alive(identity))
    except psutil.NoSuchProcess:return False



def native_app_window(device,identity):
    """Revalidate the held main window using only OS identity, never late title IPC."""
    require(native_app_identity_alive(device,identity),'Native app identity invalid')
    owner=identity['window']
    if sys.platform=='win32':
        import ctypes
        from ctypes import wintypes
        user=ctypes.WinDLL('user32',use_last_error=True)
        user.IsWindow.argtypes=[wintypes.HWND];user.IsWindow.restype=wintypes.BOOL
        user.GetWindowThreadProcessId.argtypes=[wintypes.HWND,ctypes.POINTER(wintypes.DWORD)]
        user.GetWindowThreadProcessId.restype=wintypes.DWORD
        require(bool(user.IsWindow(owner.handle)),'Native app identity invalid')
        pid=wintypes.DWORD();user.GetWindowThreadProcessId(owner.handle,ctypes.byref(pid));pid=pid.value
    else:
        result=subprocess.run(['xdotool','getwindowpid',str(owner)],env=device.player_wm.env,
            capture_output=True,text=True,timeout=3,check=True)
        require(len(result.stdout)<=32 and result.stdout.strip().isdecimal(),'Native app identity invalid')
        pid=int(result.stdout.strip())
    require(pid==identity['pid'] and native_app_identity_alive(device,identity),'Native app identity invalid')
    return owner


def native_app_identity_self_test():
    args=['/fixture/nexusbro'];family={100,200,300}
    assert valid_native_app_binding(200,100,family,True,args,'20','20','10','10')
    assert valid_native_app_binding(100,100,{100},True,args,'10','10','10','10')
    assert not valid_native_app_binding(999,100,family,True,args,'20','20','10','10')
    assert not valid_native_app_binding(200,100,family,False,args,'20','20','10','10')
    for flag in ('--nb-dvizhok','--nb-dvizhok=1','--type=renderer','--type'):
        assert not valid_native_app_binding(200,100,family,True,args+[flag],'20','20','10','10')
    assert not valid_native_app_binding(200,100,family,True,args,'20','21','10','10')
    assert not valid_native_app_binding(200,100,family,True,args,'20','20','10','11')
    assert not valid_native_app_binding(200,100,set(range(1,130)),True,args,'20','20','10','10')
    assert not valid_native_app_binding(200,100,{100,200,'foreign'},True,args,'20','20','10','10')
    receipt=dict(schema=1,prepared_update=True,parent_pid=200,owners=1,clean=1,forced=0,
                 unacknowledged=0,unclean=0,pending_starts=0,complete=True)
    assert parse_shutdown_receipt(receipt,200)==receipt
    try:parse_shutdown_receipt(receipt,100)
    except Exception:pass
    else:raise AssertionError('Launcher PID accepted as native receipt owner')
    return dict(self_test=True,synthetic_only=True,native_execution=False)


def foreground(device):
    if sys.platform=='win32' or not getattr(device,'player_wm',None):return bare_foreground(device)
    owner=native_owner(device)
    subprocess.run(['xdotool','windowactivate','--sync',owner],env=device.player_wm.env,check=True,timeout=5)
    painted(device.client)


def switch_super_foreground(client,device,identifier):
    # The sidebar belongs to the native shell owner, not a previously focused
    # CEF child. Establish ownership before the one ordinary physical click.
    foreground(device)
    switch_super(client,identifier)


def minimize_actual(device, minimized):
    owner=native_owner(device)
    if sys.platform=='win32':
        import ctypes
        if minimized:owner.minimize()
        else:owner.restore();owner.set_focus()
        wait_for(lambda:bool(ctypes.windll.user32.IsIconic(owner.handle))==minimized,
                 'Native minimize receipt missing',5)
    else:
        env=device.player_wm.env
        action=['windowminimize',owner] if minimized else ['windowmap',owner,'windowactivate','--sync',owner]
        subprocess.run(['xdotool',*action],env=env,check=True,timeout=5)
        def expected():
            value=subprocess.check_output(['xprop','-id',owner,'WM_STATE'],env=env,text=True,timeout=3)
            return ('Iconic' if minimized else 'Normal') in value
        wait_for(expected,'Native minimize receipt missing',5)
    return True


RESTORE_OBSERVER_JS = r"""(expected=>{
 const internal=window.__TAURI_INTERNALS__,original=internal?.invoke;
 if(typeof original!=="function"||window.__nbRestoreObservation)return false;
 const descriptor=Object.getOwnPropertyDescriptor(internal,"invoke");
 const invoke_descriptor={present:!!descriptor,writable:descriptor?.writable===true,configurable:descriptor?.configurable===true,getter:typeof descriptor?.get==="function",setter:typeof descriptor?.set==="function"};
 const bounded=x=>Math.min(32,x+1),event={};
 for(const type of ["pointerdown","pointerup","click"])event[type]={total:0,trusted:0,owner_match:0,trusted_owner_match:0};
 const ipc={pereklyuchit:{called:0,expected:0,resolved:0,rejected:0,ok_true:0,ok_false:0,ok_unknown:0,string_error:0,object_error:0,other_error:0},fokus_obolochke:{called:0,expected:0,resolved:0,rejected:0,ok_true:0,ok_false:0,ok_unknown:0,string_error:0,object_error:0,other_error:0}};
 let attached_failed=0,synchronous_error=0;
 const listener=e=>{const v=event[e.type],owner=e.target?.closest?.('.vkladka[data-super-id]');const match=owner?.dataset.superId===expected;v.total=bounded(v.total);if(Number.isFinite(e.clientX)&&Number.isFinite(e.clientY)){v.last_client_x=Math.round(e.clientX);v.last_client_y=Math.round(e.clientY);}if(e.isTrusted)v.trusted=bounded(v.trusted);if(match)v.owner_match=bounded(v.owner_match);if(match&&e.isTrusted)v.trusted_owner_match=bounded(v.trusted_owner_match);};
 const error=(v,e)=>{v.rejected=bounded(v.rejected);const k=typeof e==="string"?"string_error":e!==null&&typeof e==="object"?"object_error":"other_error";v[k]=bounded(v[k]);};
 function wrapped(){
  const name=arguments[0],v=name==="pereklyuchit"||name==="fokus_obolochke"?ipc[name]:null;
  if(v){v.called=bounded(v.called);if(name==="fokus_obolochke"||arguments[1]?.id===expected)v.expected=bounded(v.expected);}
  let promise;try{promise=Reflect.apply(original,this,arguments);}catch(e){if(v){synchronous_error=bounded(synchronous_error);error(v,e);}throw e;}
  if(v){try{Promise.prototype.then.call(promise,value=>{v.resolved=bounded(v.resolved);const k=value?.ok===true?"ok_true":value?.ok===false?"ok_false":"ok_unknown";v[k]=bounded(v[k]);},e=>{error(v,e);});}catch(_){attached_failed=bounded(attached_failed);}}
  return promise;
 }
 for(const type of Object.keys(event))document.addEventListener(type,listener,true);
 let invoke_wrapped=false,assignment_threw=false;try{internal.invoke=wrapped;invoke_wrapped=internal.invoke===wrapped;}catch(_){assignment_threw=true;}
 window.__nbRestoreObservation={finish:()=>{try{if(invoke_wrapped)internal.invoke=original;}catch(_){}for(const type of Object.keys(event))document.removeEventListener(type,listener,true);delete window.__nbRestoreObservation;return {events:event,ipc,attached_failed,synchronous_error,pointer_listeners_installed:true,invoke_wrapped,invoke_descriptor,assignment_threw,invoke_restored:internal.invoke===original,listeners_removed:true};}};
 return {pointer_listeners_installed:true,invoke_wrapped,invoke_descriptor,assignment_threw};
})"""


def restore_observer_self_test():
    script="""const assert=require('node:assert/strict');(async()=>{
    const handlers=new Map(),owner='player-b',receiver={};let result=Promise.resolve({ok:true}),seen;
    function original(){seen={receiver:this,args:[...arguments]};return result;}
    global.window={__TAURI_INTERNALS__:{invoke:original}};
    global.document={addEventListener:(k,f)=>handlers.set(k,f),removeEventListener:(k,f)=>{assert.equal(handlers.get(k),f);handlers.delete(k);}};
    const install="""+RESTORE_OBSERVER_JS+""";
    const setup=install(owner);assert.equal(setup.invoke_wrapped,true);assert.equal(setup.pointer_listeners_installed,true);const wrapped=window.__TAURI_INTERNALS__.invoke,args={id:owner};
    const actual=wrapped.call(receiver,'pereklyuchit',args);assert.equal(actual,result);assert.equal(seen.receiver,receiver);assert.equal(seen.args[1],args);
    handlers.get('click')({type:'click',isTrusted:true,target:{closest:()=>({dataset:{superId:owner}})}});
    await actual;result=Promise.reject('PRIVATE https://secret.invalid/token');const rejected=wrapped.call(receiver,'fokus_obolochke');assert.equal(rejected,result);await rejected.catch(()=>{});
    const value=window.__nbRestoreObservation.finish();assert.equal(window.__TAURI_INTERNALS__.invoke,original);assert.equal(handlers.size,0);assert.equal(window.__nbRestoreObservation,undefined);
    assert.equal(value.events.click.trusted_owner_match,1);assert.equal(value.ipc.pereklyuchit.resolved,1);assert.equal(value.ipc.pereklyuchit.ok_true,1);assert.equal(value.ipc.fokus_obolochke.rejected,1);
    assert.equal(JSON.stringify(value).includes('PRIVATE'),false);
    const frozen={};Object.defineProperty(frozen,'invoke',{value:original,writable:false,configurable:false});window.__TAURI_INTERNALS__=frozen;
    const unavailable=install(owner);assert.equal(unavailable.invoke_wrapped,false);assert.equal(unavailable.pointer_listeners_installed,true);
    handlers.get('pointerdown')({type:'pointerdown',isTrusted:true,target:{closest:()=>({dataset:{superId:owner}})}});
    const independent=window.__nbRestoreObservation.finish();assert.equal(independent.events.pointerdown.trusted_owner_match,1);assert.equal(handlers.size,0);assert.equal(frozen.invoke,original);
    console.log(JSON.stringify({self_test:true,native_execution:false}));
    })().catch(()=>{process.stderr.write('restore_observer_self_test_failed\\n');process.exitCode=1;});"""
    result=subprocess.run(['node','-e',script],capture_output=True,text=True,timeout=10)
    require(result.returncode==0,'Restore observer promise/cleanup self-test failed')
    return {'self_test':True,'native_execution':False}


def xwin_geometry(text):
    import re
    def number(label):
        value=re.search(r'^\s*'+re.escape(label)+r':\s*(-?\d+)',text,re.M)
        return int(value[1]) if value else None
    return {'x':number('Absolute upper-left X'),'y':number('Absolute upper-left Y'),
            'width':number('Width'),'height':number('Height')}


def input_geometry(client,device,selector):
    """Read-only bounded private-X11 receipt; no activation or coordinate fix."""
    if not sys.platform.startswith('linux') or not getattr(device,'player_wm',None):return {'available':False}
    import re,psutil
    result={'available':False,'webview_origin_proven':False}
    try:
        env=device.player_wm.env
        def read(*args):
            value=subprocess.run(args,env=env,capture_output=True,text=True,timeout=3)
            require(value.returncode==0 and len(value.stdout)<=65536,'Read-only X11 geometry unavailable')
            return value.stdout
        dom=client.js('(()=>{const e=document.querySelector('+json.dumps(selector)+'),r=e?.getBoundingClientRect();return r?{x:Math.round(r.x+r.width/2),y:Math.round(r.y+r.height/2),scale:devicePixelRatio||1}:null})()')
        require(isinstance(dom,dict) and all(type(dom.get(k))in(int,float) for k in ('x','y','scale')),'Geometry target unavailable')
        origin=call(client,'plugin:window|inner_position',label=client.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label'))
        expected=(round(origin['x']+dom['x']*dom['scale']),round(origin['y']+dom['y']*dom['scale']))
        owner=str(native_owner(device));tree=read('xwininfo','-id',owner,'-stats','-tree');outer=xwin_geometry(tree)
        require(all(type(v)is int for v in outer.values()),'X11 owner geometry unavailable')
        children=[]
        for line in tree.splitlines():
            match=re.search(r'(\d+)x(\d+)([+-]\d+)([+-]\d+)\s+([+-]\d+)([+-]\d+)\s*$',line)
            if match and len(children)<16:
                w,h,_,_,x,y=map(int,match.groups());children.append({'x_offset':x-outer['x'],'y_offset':y-outer['y'],'width':w,'height':h,'contains_expected_click':x<=expected[0]<x+w and y<=expected[1]<y+h})
        result.update(available=True,dom_center=dom,inner_origin={'x':origin['x'],'y':origin['y']},outer=outer,
                      expected_click={'x':expected[0],'y':expected[1]},inner_minus_outer={'x':origin['x']-outer['x'],'y':origin['y']-outer['y']},native_children=children)
        mouse=dict(line.split('=',1) for line in read('xdotool','getmouselocation','--shell').splitlines() if '=' in line)
        root_pid=int(read('xdotool','getwindowpid',owner).strip())
        owned={p.pid for p in psutil.Process(device.app.pid).children(recursive=True)}|{device.app.pid}
        def role(window):
            try:
                pid=int(read('xdotool','getwindowpid',window).strip())
                if pid==root_pid:return 'root'
                if pid not in owned:return 'other'
                args=psutil.Process(pid).cmdline()
                return 'cef' if any(a=='--nb-dvizhok' or a.startswith('--type=') for a in args) else 'owned_other'
            except Exception:return 'unavailable'
        pointer=mouse.get('WINDOW','0');pointer_rect=xwin_geometry(read('xwininfo','-id',pointer,'-stats'))
        result.update(available=True,dom_center=dom,inner_origin={'x':origin['x'],'y':origin['y']},outer=outer,
                      expected_click={'x':expected[0],'y':expected[1]},inner_minus_outer={'x':origin['x']-outer['x'],'y':origin['y']-outer['y']},
                      native_children=children,pointer={'x':int(mouse['X']),'y':int(mouse['Y']),'window_role':role(pointer),'window_geometry':pointer_rect},
                      focus_role=role(read('xdotool','getwindowfocus').strip()))
    except Exception:result['read_incomplete']=True
    return result


def sidebar_crop_box(outer,screen,expected):
    """Private sidebar only; never address bar or native page content."""
    if not all(type(outer.get(k))is int for k in ('x','y','width','height')):return None
    if outer['width']<176 or outer['height']<320:return None
    x,y=outer['x'],outer['y'];sw,sh=screen
    box=(max(0,x),max(0,y),min(sw,x+176),min(sh,y+320))
    if not (0<=box[0]<box[2]<=sw and 0<=box[1]<box[3]<=sh):return None
    if not (box[0]<=expected['x']<box[2] and box[1]<=expected['y']<box[3]):return None
    return box


def sidebar_frame(device,geometry):
    from PIL import ImageGrab
    require(sys.platform.startswith('linux') and os.environ.get('GITHUB_ACTIONS')=='true','Sidebar diagnostic gate closed')
    require(getattr(device,'player_sidebar_testdata',None)==device.data.resolve()
            and getattr(device,'player_sidebar_out',None) is not None,'Sidebar fixture gate closed')
    require(getattr(device,'x',None) is not None and device.x.poll() is None,'Owned Xvfb unavailable')
    pid=device.app.pid;birth=kernel_birth(pid,'linux_ticks')
    require(device.app.poll() is None and birth is not None,'Sidebar owner unavailable')
    owner=str(native_owner(device))
    env=device.player_wm.env
    text=subprocess.check_output(['xwininfo','-id',owner,'-stats'],env=env,text=True,stderr=subprocess.DEVNULL,timeout=3)
    require(len(text)<=16384,'Sidebar geometry outside bounds')
    outer=xwin_geometry(text);expected=geometry['expected_click']
    children=geometry.get('native_children',[])
    require(bool(children) and all(type(c.get('x_offset'))is int and c['x_offset']>=176 for c in children),
            'Sidebar native content boundary unavailable')
    require(geometry.get('inner_minus_outer',{}).get('x')==0 and geometry.get('dom_center',{}).get('scale')==1,
            'Sidebar fixture scale unavailable')
    with ImageGrab.grab(xdisplay=device.display) as image:
        box=sidebar_crop_box(outer,image.size,expected)
        require(box is not None,'Sidebar crop outside safe bounds')
        cropped=image.crop(box)
    try:
        require(device.app.poll() is None and kernel_birth(pid,'linux_ticks')==birth
                and str(native_owner(device))==owner,'Sidebar owner changed')
        receipt={'width':cropped.width,'height':cropped.height,'owned_window_verified':True,
                 'expected_point_inside':True,'expected_relative':{'x':expected['x']-box[0],'y':expected['y']-box[1]},
                 'relative_box':{'x':box[0]-outer['x'],'y':box[1]-outer['y'],
                                 'width':cropped.width,'height':cropped.height}}
        return cropped,receipt,(pid,birth,owner)
    except Exception:
        cropped.close();raise


def sidebar_before(device,case):
    if not sys.platform.startswith('linux'):return None
    try:
        before=sidebar_frame(device,case['restore_geometry_before'])
        case['sidebar_paint']={'available':False,'before_captured':True}
        return before
    except Exception:
        case['sidebar_paint']={'available':False,'reason':'before_unavailable'}
        return None


def sidebar_failed(device,case,before):
    """Never masks the actual restore failure; no input/focus/activation calls."""
    if before is None:return
    import io,hashlib
    after=None
    try:
        after=sidebar_frame(device,case['restore_geometry_before'])
        require(after[2]==before[2],'Sidebar owner changed between captures')
        frames=[]
        for name,item in [('before',before),('failure',after)]:
            image,receipt,_identity=item
            with io.BytesIO() as buffer:
                image.save(buffer,format='PNG');data=buffer.getvalue()
            require(len(data)<=256*1024,'Sidebar PNG outside bounds')
            basename='background-super-B-'+name+'.png'
            with (device.player_sidebar_out/basename).open('xb') as output:output.write(data)
            frames.append({**receipt,'file':basename,'bytes':len(data),'sha256':hashlib.sha256(data).hexdigest()})
        case['sidebar_paint']={'available':True,'failure_only':True,'frames':frames}
    except Exception:
        case['sidebar_paint']={'available':False,'reason':'failure_capture_unavailable'}
    finally:
        if after is not None:after[0].close()


def sidebar_self_test():
    assert sidebar_crop_box({'x':1,'y':20,'width':1280,'height':820},(1280,900),{'x':91,'y':176})==(1,20,177,340)
    assert sidebar_crop_box({'x':-5,'y':-10,'width':1280,'height':820},(1280,900),{'x':90,'y':156})==(0,0,171,310)
    assert sidebar_crop_box({'x':1200,'y':20,'width':1280,'height':820},(1280,900),{'x':1290,'y':176}) is None
    assert sidebar_crop_box({'x':1,'y':20,'width':1280,'height':820},(1280,900),{'x':180,'y':176}) is None
    assert sidebar_crop_box({'x':1,'y':20,'width':175,'height':820},(1280,900),{'x':91,'y':176}) is None
    # Disabled/missing fixture authority yields no capture or output file.
    class Empty:pass
    case={};assert sidebar_before(Empty(),case) is None
    import tempfile
    class Frame:
        def save(self,output,format):assert format=='PNG';output.write(b'fake-png')
        def close(self):pass
    global sidebar_frame
    original=sidebar_frame
    try:
        with tempfile.TemporaryDirectory() as folder:
            device=Empty();device.player_sidebar_out=pathlib.Path(folder)
            before=(Frame(),{'width':176,'height':320},(1,2,3))
            # Capturing RAM alone never persists the success path.
            assert not list(device.player_sidebar_out.iterdir())
            sidebar_frame=lambda *_:(Frame(),{'width':176,'height':320},(1,2,3))
            case={'restore_geometry_before':{}};sidebar_failed(device,case,before)
            assert case['sidebar_paint']['available'] is True
            assert sorted(p.name for p in device.player_sidebar_out.iterdir())==[
                'background-super-B-before.png','background-super-B-failure.png']
            sidebar_frame=lambda *_:(_ for _ in ()).throw(ValueError('private-error-not-exported'))
            failed={};sidebar_failed(device,failed,before)
            assert failed=={'sidebar_paint':{'available':False,'reason':'failure_capture_unavailable'}}
    finally:sidebar_frame=original
    return {'crop_bounds':True,'expected_point_bound':True,'fixture_gate':True,'failure_only_save':True,'error_redacted':True}


def restore_observer_begin(client,identifier,case,device=None):
    try:
        setup=client.js(RESTORE_OBSERVER_JS+'('+json.dumps(identifier)+')')
        case['restore_event_observer_installed']=isinstance(setup,dict) and setup.get('invoke_wrapped') is True
        if isinstance(setup,dict):case['restore_observer_setup']=setup
    except Exception:case['restore_event_observer_installed']=False
    if device is not None:case['restore_geometry_before']=input_geometry(client,device,'.vkladka[data-super-id="'+identifier+'"]')


def restore_observer_finish(client,case,device=None,identifier=None):
    try:
        value=client.js('window.__nbRestoreObservation?.finish()??null')
        if isinstance(value,dict):case['restore_input_observation']=value
        else:case['restore_event_observer_incomplete']=True
    except Exception:case['restore_event_observer_incomplete']=True
    if device is not None:case['restore_geometry_after']=input_geometry(client,device,'.vkladka[data-super-id="'+identifier+'"]')


def background_receipt(client,device,identifier,tab,hide,restore,kind,diagnostics):
    import math
    cases=diagnostics.setdefault('background_cases',[])
    require(len(cases)<6,'Background diagnostic case bound exceeded')
    case={'name':kind,'snapshots':[],'hide_complete':False,'restore_complete':False,'restore_callback_entered':False}
    cases.append(case);started=time.monotonic()
    fields={'configured','disposed','playing','separate','position','audio_position','duration','skew',
            'decoded','dropped','video_width','video_height'}
    visibility_fields={'hidden','video_paused','audio_paused'}
    def snapshot_record(phase,state):
        require(len(case['snapshots'])<8,'Background diagnostic snapshot bound exceeded')
        safe={key:value for key,value in state.items() if
              (key in fields and (type(value) is bool or type(value) in (int,float) and math.isfinite(value)))
              or (key in visibility_fields and type(value) is bool)}
        extended=all(type(state.get(key)) is bool for key in visibility_fields)
        frame={'phase':phase,'elapsed_ms':round((time.monotonic()-started)*1000),
               'state':safe,'extended_state_available':extended}
        if case['snapshots']:
            previous_frame=case['snapshots'][-1]
            frame.update(decoded_delta=state['decoded']-previous_frame['state']['decoded'],
                         audio_delta=round(state['audio_position']-previous_frame['state']['audio_position'],6),
                         wall_delta_ms=frame['elapsed_ms']-previous_frame['elapsed_ms'])
        case['extended_state_available']=case.get('extended_state_available',True) and extended
        case['snapshots'].append(frame)
    before=observe(client,identifier,tab,diagnostics=diagnostics)
    snapshot_record('before',before)
    row_before=next(r for r in rows(client,identifier)['vkladki'] if r['id']==tab)
    def row_record():
        session=rows(client,identifier);native=session['vkladki']
        target=next((row for row in native if row['id']==tab),None)
        return {'tab_count':len(native),'live_tab_count':sum(row.get('browser_id',0)>0 for row in native),
                'target_present':target is not None,'active_is_target':session['aktivnaya']==tab,
                'target_browser_same':bool(target and target.get('browser_id')==row_before['browser_id']),
                'target_loading':bool(target and target.get('zagruzka')),
                'target_deferred':bool(target and target.get('otlozhena')),
                'selected_super_is_owner':call(client,'aktivnaya')==identifier}
    case['rows_before']=row_record()
    require(before.get('playing'),'Background test requires actual playback')
    samples=[]
    try:
        hide();case['hide_complete']=True
        case['rows_hidden']=row_record()
        previous=before['audio_position']
        for _ in range(6):
            time.sleep(2)
            current=observe(client,identifier,tab,diagnostics=diagnostics)
            snapshot_record('hidden',current)  # Retained even when this assertion fails.
            case['snapshots'][-1]['rows']=row_record()
            case['last_audio_delta']=round(current['audio_position']-previous,6)
            case['hidden_decoded_delta']=current['decoded']-before['decoded']
            case['hidden_audio_delta']=round(current['audio_position']-before['audio_position'],6)
            case['observed_hidden_seconds']=round((case['snapshots'][-1]['elapsed_ms']-case['snapshots'][0]['elapsed_ms'])/1000,3)
            require(not current.get('disposed') and current['audio_position']>previous+.5,
                    'Background audio froze or paused')
            # Chromium may suspend hidden video decoding. No hidden video-growth assertion.
            samples.append(current['audio_position']);previous=current['audio_position']
    finally:
        case['restore_callback_entered']=True
        observe_restore=kind.startswith('background-super-')
        if observe_restore:restore_observer_begin(client,identifier,case,device)
        sidebar=sidebar_before(device,case) if kind=='background-super-B' else None
        try:
            restore();case['restore_complete']=True
        except Exception:
            sidebar_failed(device,case,sidebar)
            try:
                # Read-only shell identity/geometry; no retry or focus mutation.
                selector='.vkladka[data-super-id="'+identifier+'"]'
                case['restore_dom']=client.js('''(()=>{const e=document.querySelector('''+json.dumps(selector)+'''),r=e?.getBoundingClientRect(),x=r?r.x+r.width/2:0,y=r?r.y+r.height/2:0,h=r?document.elementFromPoint(x,y):null;return {active_super_match:!!e?.classList.contains('aktivna'),selected_tab_count:document.querySelectorAll('.obychnaya[aria-selected=true]').length,super_count:document.querySelectorAll('.vkladka[data-super-id]').length,target_count:document.querySelectorAll('''+json.dumps(selector)+''').length,document_focused:document.hasFocus(),target_visible:!!r&&r.width>0&&r.height>0,hit_target:!!e&&!!h&&(e===h||e.contains(h))}})()''')
                case['restore_rows']=row_record()
                if sys.platform!='win32' and getattr(device,'player_wm',None):
                    intended=native_owner(device)
                    focused=subprocess.check_output(['xdotool','getwindowfocus'],env=device.player_wm.env,timeout=3,text=True).strip()
                    case['restore_native_focus_matches']=focused==str(intended)
            except Exception:
                case['restore_diagnostic_incomplete']=True
            raise
        finally:
            if sidebar is not None:sidebar[0].close()
            if observe_restore:restore_observer_finish(client,case,device,identifier)
        case['rows_restored']=row_record()
        restored=observe(client,identifier,tab,diagnostics=diagnostics)
        snapshot_record('after_restore',restored)
    foreground(device)
    first=restored
    paint_before=video_paint(client,device);time.sleep(2)
    after=observe(client,identifier,tab,diagnostics=diagnostics)
    paint_after=video_paint(client,device)
    require(after.get('playing') and after['decoded']>first['decoded'] and after['skew']<1,
            'Background return did not automatically resume synchronized decode')
    require(sum(a!=b for a,b in zip(paint_before,paint_after))>30,'Actual OS video paint did not change')
    row_after=next(r for r in rows(client,identifier)['vkladki'] if r['id']==tab)
    require(row_before['browser_id']==row_after['browser_id'],'Background playback browser changed')
    return {'name':kind,'wall_seconds':case['observed_hidden_seconds'],'nominal_sleep_seconds':12,'audio_samples':6,
            'hidden_decoded_delta':case['hidden_decoded_delta'],'extended_state_available':case['extended_state_available'],
            'audio_advance':round(samples[-1]-before['audio_position'],3),
            'return_decoded_advance':after['decoded']-first['decoded'],
            'return_skew':after['skew'],'return_paint_changed':True,
            'browser_unchanged':True,'hidden_decode_growth_required':False}


def certificates(root, hosts=None):
    from cryptography import x509
    from cryptography.hazmat.primitives import hashes, serialization
    from cryptography.hazmat.primitives.asymmetric import rsa
    from cryptography.x509.oid import NameOID, ExtendedKeyUsageOID
    names=[HOST] if hosts is None else list(hosts)
    require(0<len(names)<=2 and len(set(names))==len(names) and all(host in (HOST,CAPTION_HOST) for host in names),
            'Fixture certificate host invalid')
    root.mkdir()
    key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    leafkey = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, 'Disposable player fixture CA')])
    now = datetime.datetime.now(datetime.timezone.utc)
    def builder(subject, public):
        return (x509.CertificateBuilder().subject_name(subject).issuer_name(name).public_key(public)
                .serial_number(x509.random_serial_number()).not_valid_before(now-datetime.timedelta(minutes=2))
                .not_valid_after(now+datetime.timedelta(days=1)))
    ca = (builder(name, key.public_key()).add_extension(x509.BasicConstraints(ca=True, path_length=0), True)
          .add_extension(x509.KeyUsage(False,False,False,False,False,True,True,False,False),True)
          .add_extension(x509.SubjectKeyIdentifier.from_public_key(key.public_key()),False).sign(key, hashes.SHA256()))
    leaf = (builder(x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, HOST)]), leafkey.public_key())
            .add_extension(x509.BasicConstraints(ca=False, path_length=None), True)
            .add_extension(x509.SubjectAlternativeName([x509.DNSName(host) for host in names]), False)
            .add_extension(x509.ExtendedKeyUsage([ExtendedKeyUsageOID.SERVER_AUTH]), False)
            .add_extension(x509.KeyUsage(True,False,True,False,False,False,False,False,False),True)
            .add_extension(x509.AuthorityKeyIdentifier.from_issuer_public_key(key.public_key()),False).sign(key, hashes.SHA256()))
    for filename, data in [('ca.crt', ca.public_bytes(serialization.Encoding.PEM)),
                           ('server.crt', leaf.public_bytes(serialization.Encoding.PEM)),
                           ('server.key', leafkey.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8, serialization.NoEncryption()))]:
        target = root / filename
        with target.open('xb') as stream: stream.write(data)
        target.chmod(0o600)
    return ca.fingerprint(hashes.SHA1()).hex().upper()


class Trust:
    def __init__(self, root, cert, thumb):
        self.root, self.cert, self.thumb = root, cert, thumb
        self.oldhome = os.environ.get('HOME'); self.installed = False; self.windows = None
    def install(self):
        if sys.platform == 'win32':
            self.windows = WindowsFixtureTrust(self.root,self.cert,self.cert.parent/'server.crt')
            self.windows.install()
        else:
            home = self.root / 'home'; database = home / '.pki' / 'nssdb'; database.mkdir(parents=True)
            for args in (['-N','--empty-password'], ['-A','-n','Disposable player CA','-t','C,,','-i',str(self.cert)]):
                subprocess.run(['certutil','-d','sql:'+str(database),*args],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=15)
            os.environ['HOME'] = str(home)
        self.installed = True
    def close(self):
        if self.windows:
            self.windows.close()
        elif self.oldhome is None: os.environ.pop('HOME',None)
        else: os.environ['HOME'] = self.oldhome


class Server(socketserver.ThreadingTCPServer):
    daemon_threads = True
    allow_reuse_address = False


class Fixture:
    def __init__(self, cert, key, video, audio, media_sentinel=None, positive_sentinel=None, *, video_high=None, caption_body=None):
        self.lock = threading.Lock(); self.peers = {}; self.receipts = []; self.connects = []; self.direct = 0
        self.servers = []; self.sockets = set(); self.threads = []
        self.stalls = {}; self.closing = threading.Event()
        self.media_sentinel=media_sentinel if media_sentinel is not None else secrets.token_hex(24)
        self.positive_sentinel=positive_sentinel if positive_sentinel is not None else secrets.token_hex(24)
        self.canary_receipts=[]
        if caption_body is not None:
            require(type(caption_body) is bytes and 0<len(caption_body)<=65536 and caption_body.startswith(b'WEBVTT'),
                    'Caption fixture body invalid')
            caption_body.decode('utf-8')
        self.caption_body=caption_body
        self.payload = {'video': video.read_bytes(), 'audio': audio.read_bytes()}
        if video_high is not None:self.payload['video_high']=video_high.read_bytes()
        require(all(0 < len(v) <= 32*1024*1024 for v in self.payload.values()), 'Media fixture size invalid')
        owner = self
        class Origin(http.server.BaseHTTPRequestHandler):
            def log_message(self,*_): pass
            def setup(self):
                super().setup()
                self.connection_lane=getattr(self.connection,'fixture_lane','Direct')
            def do_GET(self):
                path = urllib.parse.urlsplit(self.path); params=urllib.parse.parse_qs(path.query,keep_blank_values=True)
                track = params.get('track',[''])[0]
                request_host=self.headers.get('Host','').split(':',1)[0].strip().lower()
                if request_host==CAPTION_HOST:
                    if (owner.caption_body is None or path.path!='/api/timedtext'
                            or params!={'v':['BaW_jenozKc'],'fmt':['vtt'],'lang':['de']}):
                        self.send_error(404);return
                    with owner.lock:
                        owner.receipts.append({'kind':'caption','lane':self.connection_lane,'host':'youtube'})
                    self.send_response(200);self.send_header('Content-Type','text/vtt; charset=utf-8')
                    self.send_header('Cache-Control','no-store');self.send_header('Access-Control-Allow-Origin','*')
                    self.send_header('Content-Length',str(len(owner.caption_body)));self.end_headers()
                    try:self.wfile.write(owner.caption_body);self.wfile.flush()
                    except (OSError,ssl.SSLError):pass
                    return
                if request_host!=HOST:self.send_error(404);return
                if path.path=='/cache-canary' and not path.query:
                    # BODY-only high entropy marker: it is never in history/URL.
                    body=('<!doctype html><title>Cache canary</title><style>html,body{margin:0;background:rgb(28,112,80)}</style><main>'+owner.positive_sentinel+'</main>').encode()
                    with owner.lock:owner.canary_receipts.append(self.connection_lane)
                    self.send_response(200);self.send_header('Content-Type','text/html; charset=utf-8')
                    self.send_header('Cache-Control','private, max-age=3600')
                    self.send_header('Content-Length',str(len(body)));self.end_headers()
                    try:self.wfile.write(body);self.wfile.flush()
                    except (OSError,ssl.SSLError):pass
                    return
                if path.path != '/videoplayback' or track not in ('video','audio') or params.get('track')!=[track]:
                    self.send_error(404); return
                quality=params.get('quality',['low'])
                if len(quality)!=1 or quality[0] not in ('low','high') or track=='audio' and 'quality' in params:
                    self.send_error(404);return
                payload_key='video_high' if track=='video' and quality[0]=='high' else track
                if payload_key not in owner.payload:self.send_error(404);return
                body = owner.payload[payload_key]; start = 0; end = len(body)-1; ranged = False
                header = self.headers.get('Range')
                if header:
                    import re
                    match = re.fullmatch(r'bytes=(\d+)-(\d*)',header)
                    if not match: self.send_error(416); return
                    start = int(match[1]); end = min(int(match[2]) if match[2] else end,end); ranged=True
                    if start > end: self.send_error(416); return
                with owner.lock:
                    lane=self.connection_lane
                    policy=owner.stalls.get((lane,track))
                    receipt={'kind':'media','host':'googlevideo','lane':lane,'track':track,'range':ranged,
                             'bytes':end-start+1,'privacy_marked':params.get('nb_privacy')==[owner.media_sentinel]}
                    if track=='video':receipt['quality']=quality[0]
                    owner.receipts.append(receipt)
                self.send_response(206 if ranged else 200)
                self.send_header('Content-Type','video/webm' if track=='video' else 'audio/webm')
                self.send_header('Accept-Ranges','bytes')
                marked=urllib.parse.parse_qs(path.query).get('nb_privacy')==[owner.media_sentinel]
                self.send_header('Cache-Control','private, max-age=3600' if marked else 'no-store')
                self.send_header('Content-Length',str(end-start+1))
                if ranged:self.send_header('Content-Range',f'bytes {start}-{end}/{len(body)}')
                self.end_headers()
                try:
                    if policy is None:
                        self.wfile.write(body[start:end+1])
                    else:
                        # Cues/SeekHead at the tail may be required before the
                        # first frame. This bounded metadata window cannot fill
                        # the continuous beginning of a 120-second media track.
                        tail_metadata=start>=max(0,len(body)-16384) and end-start+1<=16384
                        for offset in range(start,end+1,4096):
                            if owner.closing.is_set():return
                            if not tail_metadata:
                                blocked=False
                                while not policy['gate'].wait(.1):
                                    if owner.closing.is_set():return
                                    if not blocked:
                                        with owner.lock:policy['blocked_writers']+=1
                                        blocked=True
                            chunk=body[offset:min(offset+4096,end+1)]
                            self.wfile.write(chunk);self.wfile.flush()
                            with owner.lock:
                                policy['sent_bytes']+=len(chunk)
                                if tail_metadata:policy['metadata_bytes']+=len(chunk)
                            # ~2x real-time: enough to start, not enough to
                            # prebuffer all 120 seconds before gate admission.
                            if not tail_metadata and owner.closing.wait(len(chunk)*60/len(body)):return
                except (OSError,ssl.SSLError):pass
        origin = http.server.ThreadingHTTPServer(('127.0.0.1',0),Origin); origin.daemon_threads=True
        tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER); tls.load_cert_chain(cert,key)
        def bind_tls_lane(connection, _server_name, _context):
            # CONNECT registers its upstream before admitting client TLS. Capture
            # identity during handshake, before later EOF can remove that map.
            with owner.lock:
                connection.fixture_lane=owner.peers.get(connection.getpeername()[1],'Direct')
        tls.sni_callback=bind_tls_lane
        origin.socket=tls.wrap_socket(origin.socket,server_side=True); self.origin_port=origin.server_port; self.start(origin)
        self.proxies={}
        for lane in ('A','B'):
            class Proxy(socketserver.StreamRequestHandler):
                def handle(self):
                    self.connection.settimeout(10)
                    try:
                        first=self.rfile.readline(1025)
                        allowed=[HOST]+([CAPTION_HOST] if owner.caption_body is not None else [])
                        if first not in [('CONNECT '+host+':443 HTTP/1.1\r\n').encode() for host in allowed]:return
                        total=0
                        while True:
                            line=self.rfile.readline(2049); total+=len(line)
                            if not line or total>8192:return
                            if line==b'\r\n':break
                        upstream=socket.create_connection(('127.0.0.1',origin.server_port),5)
                        peer_port=upstream.getsockname()[1]
                        with owner.lock:
                            owner.peers[peer_port]=self.server.lane
                            owner.connects.append(self.server.lane);owner.sockets.update((upstream,self.connection))
                        try:
                            self.wfile.write(b'HTTP/1.1 200 Connection Established\r\n\r\n');self.wfile.flush()
                            while True:
                                ready,_,_=select.select([self.connection,upstream],[],[],1)
                                for source in ready:
                                    data=source.recv(65536)
                                    if not data:return
                                    (upstream if source is self.connection else self.connection).sendall(data)
                        finally:
                            with owner.lock:owner.peers.pop(peer_port,None);owner.sockets.discard(upstream);owner.sockets.discard(self.connection)
                            upstream.close()
                    except OSError:pass
            server=Server(('127.0.0.1',0),Proxy);server.lane=lane;self.proxies[lane]=server.server_address[1];self.start(server)
        class Trap(socketserver.BaseRequestHandler):
            def handle(self):
                with owner.lock:owner.direct+=1
        # Never take another process' port or modify host networking policy.
        trap=Server(('127.0.0.1',443),Trap);self.start(trap)
    def start(self,server):
        self.servers.append(server);thread=threading.Thread(target=server.serve_forever,daemon=True);self.threads.append(thread);thread.start()
    def stall_policy(self,lane,track):
        gate=threading.Event();gate.set()
        policy={'gate':gate,'sent_bytes':0,'metadata_bytes':0,'blocked_writers':0}
        with self.lock:self.stalls[(lane,track)]=policy
        return policy
    def retire_stall(self,lane,track,policy):
        policy['gate'].set()
        with self.lock:
            if self.stalls.get((lane,track)) is policy:self.stalls.pop((lane,track))
    def close(self):
        self.closing.set()
        with self.lock:
            for policy in self.stalls.values():policy['gate'].set()
            for sock in list(self.sockets):
                try:sock.shutdown(socket.SHUT_RDWR)
                except OSError:pass
        for server in self.servers:server.shutdown();server.server_close()
        for thread in self.threads:thread.join(timeout=3)
    def fail_proxies(self):
        """Fail the approved routes while retaining the direct-attempt witness."""
        ports=set(self.proxies.values())
        for server in self.servers:
            if server.server_address[1] in ports:
                server.shutdown();server.server_close()
        with self.lock:
            sockets=list(self.sockets)
        for sock in sockets:
            try:sock.shutdown(socket.SHUT_RDWR)
            except OSError:pass


def linux_fixture_ack(client, identifier, tab, diagnostics):
    """Admission receipt only; configured and physical playback remain mandatory."""
    receipt = {'received': False, 'accepted': False}
    diagnostics['fixture_admission'] = receipt
    expression = ('window.__playerFixtureResults.find(x=>x.super_id==='
                  + json.dumps(identifier) + '&&x.data?.id===' + str(tab) + ')')
    value = wait_for(lambda: client.js(expression), 'Native fixture admission acknowledgment missing', 10)
    error = value.get('data', {}).get('error')
    receipt.update(received=True, accepted=error is None)
    # Unknown native values are never published (including raw URLs/errors).
    errors = {
        'Вкладка недоступна': 'browser_missing',
        'Диагностика доступна только в CI': 'ci_only',
        'Диагностика недоступна': 'fixture_invalid',
        'Диагностические дорожки отклонены': 'tracks_rejected',
        'Нет подходящих WebM дорожек для лёгкого плеера': 'tracks_unsupported',
        'Контекст вкладки недоступен': 'context_missing',
        'Не удалось подготовить внутренний плеер': 'scheme_install_failed',
        'Не удалось открыть внутренний плеер': 'grant_failed',
        'Плеер занят': 'grant_busy', 'Плеер закрыт': 'grant_closed',
        'Плеер уже закрыт': 'grant_revoked',
        'Не удалось подготовить видео': 'config_failed',
        'Документ вкладки недоступен': 'frame_missing',
    }
    receipt['code'] = ('accepted' if error is None else
                       errors.get(error, 'unknown_native_error') if isinstance(error, str)
                       else 'invalid_native_error')
    current = rows(client, identifier)
    selected = next((r for r in current.get('vkladki', []) if r.get('id') == tab), {})
    receipt['tab'] = {
        'present': bool(selected), 'selected': current.get('aktivnaya') == tab,
        'native_browser': isinstance(selected.get('browser_id'), int) and selected['browser_id'] > 0,
        'loading': selected.get('zagruzka') is True,
        'deferred': selected.get('otlozhena') is True,
    }
    require(error is None, 'Native fixture admission refused')


def observe(client, identifier, tab, *, admitting=False, diagnostics=None):
    client.js('window.__playerObserved=[];true')
    call(client,'obychnaya_komanda',id=identifier,komanda={'deystvie':'youtube_snapshot','id':tab})
    expression='window.__playerObserved.find(x=>x.super_id==='+json.dumps(identifier)+'&&x.data?.id==='+str(tab)+')'
    value=wait_for(lambda:client.js(expression),'Native player observation missing',10)
    if diagnostics is not None:diagnostics['native_probe_unavailable']='state' not in value.get('data',{})
    if diagnostics is not None:
        code = value.get('data', {}).get('code')
        allowed = {'probe_wrong_thread', 'browser_invalid', 'frame_absent',
                   'document_not_admitted', 'message_capacity', 'observer_unavailable',
                   'dispatch_failed', 'timeout', 'document_changed', 'cdp_method_failed',
                   'cdp_result_absent', 'snapshot_value_unavailable', 'cdp_agent_detached',
                   'snapshot_undefined', 'snapshot_exception', 'snapshot_result_absent',
                   'snapshot_not_object', 'snapshot_value_absent', 'snapshot_schema_mismatch',
                   'snapshot_field_invalid', 'snapshot_invalid_json', 'snapshot_oversized'}
        if code is not None:
            diagnostics['native_probe_code'] = code if isinstance(code, str) and code in allowed else 'unknown_probe_code'
        counters = value.get('data', {}).get('resources')
        fields = {'html', 'js', 'css', 'frame_refused'}
        if counters is not None:
            valid = (isinstance(counters, dict) and set(counters) == fields
                     and all(type(counters[k]) is int and 0 <= counters[k] <= 0xffffffff for k in fields))
            diagnostics['resources_valid'] = valid
            if valid: diagnostics['resources'] = {k: counters[k] for k in sorted(fields)}

    if admitting and isinstance(value.get('data',{}).get('error'),str) and 'state' not in value['data']:
        return {}  # only pre-admission: old document may still be navigating
    require(isinstance(value.get('data',{}).get('state'),dict),'Native observation refused')
    state=value['data']['state']
    if diagnostics is not None:
        fields={'configured','disposed','playing','separate','position','audio_position','duration','skew','decoded','dropped','video_width','video_height'}
        visibility_fields={'hidden','video_paused','audio_paused'}
        diagnostics['extended_state_available']=all(type(state.get(key)) is bool for key in visibility_fields)
        diagnostics['last_snapshot']={key:v for key,v in state.items() if
                key in fields and type(v) in (bool,int,float) or key in visibility_fields and type(v) is bool}
    return state


def focus_trace_receipt(client):
    stages={'system','navigation','unknown_source','got','take_forward','take_reverse'}
    roles={'browser_window','browser_descendant','browser_parent','parent_descendant','other','none','unknown','unsupported'}
    values=client.js('window.__cefFocusTrace??[]') or []
    return [{'stage':e['data']['stage'],'role':e['data']['role']} for e in values[-64:]
            if isinstance(e,dict) and isinstance(e.get('data'),dict)
            and e['data'].get('stage') in stages and e['data'].get('role') in roles]


def physical(client, device, kind, diagnostics):
    foreground(device);painted(client)
    # Actual CEF controls are laid out by shipped CSS: main16px padding,
    # final24px status +12px gap +button-height39px. No DOM CEF mutation.
    point=client.js('(()=>{const r=document.querySelector(".pole").getBoundingClientRect();return {x:r.x,y:r.y,w:r.width,h:r.height}})()')
    require(point['w']>=700 and point['h']>=250,'Native player viewport too small')
    x=point['x']+(65 if kind=='play' else point['w']*.45)
    y=point['y']+point['h']-65
    from PIL import ImageGrab
    label=client.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label')
    origin=call(client,'plugin:window|inner_position',label=label)
    scale=client.js('devicePixelRatio')
    # Read the button's left padding (away from text), at the same row as
    # the one physical click. Shell RAF alone is not CEF compositor proof.
    pixel=(round(origin['x']+(point['x']+23)*scale),round(origin['y']+y*scale))
    diagnostics['control']={'kind':kind,'viewport_width':round(point['w']),'viewport_height':round(point['h']),
                            'relative_click_x':round(x-point['x']),'relative_click_y':round(y-point['y']),
                            'scale':scale,'pixel_in_screen':False,'pixel_matches':False,'clicked':False}
    def controls_painted():
        with ImageGrab.grab(**({'xdisplay':device.display} if sys.platform!='win32' else {})) as image:
            inside=0<=pixel[0]<image.width and 0<=pixel[1]<image.height
            rgb=list(image.getpixel(pixel)[:3]) if inside else []
            matches=tuple(rgb) in ((41,43,59),(54,56,77))
            diagnostics['control'].update(pixel_in_screen=inside,pixel_rgb=rgb,pixel_matches=matches)
            return inside and matches
    wait_for(controls_painted,'Native player controls not physically painted',15)
    client.mouse(x,y)
    diagnostics['control']['clicked']=True


def video_paint(client, device):
    from PIL import ImageGrab
    foreground(device)
    rect=client.js('(()=>{const r=document.querySelector(".pole").getBoundingClientRect();return [r.x+r.width*.25,r.y+r.height*.3,r.x+r.width*.75,r.y+r.height*.6,devicePixelRatio]})()')
    label=client.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label')
    origin=call(client,'plugin:window|inner_position',label=label)
    box=tuple(round((origin['x'] if i%2==0 else origin['y'])+rect[i]*rect[4]) for i in range(4))
    with ImageGrab.grab(**({'xdisplay':device.display} if sys.platform!='win32' else {})) as image:
        require(box[0]>=0 and box[1]>=0 and box[2]<=image.width and box[3]<=image.height,'Physical media paint outside screen')
        return image.crop(box).resize((32,18)).convert('RGB').tobytes()


def stall_receipt(client,device,fixture,lane,identifier,tab,track,report):
    require(len(fixture.payload[track])>32768,'Stall fixture media too small for bounded metadata bypass')
    policy=fixture.stall_policy(lane,track)
    diagnostics=report['player_diagnostics']
    try:
        report['stage']='stall-load-'+track+'-'+lane
        if sys.platform.startswith('linux'):client.js('window.__playerFixtureResults=[];true')
        call(client,'obychnaya_komanda',id=identifier,komanda={'deystvie':'youtube_fixture','id':tab})
        if sys.platform.startswith('linux'):linux_fixture_ack(client,identifier,tab,diagnostics)
        wait_for(lambda:(lambda v:v if v.get('configured') and not v.get('disposed') else None)(observe(client,identifier,tab,admitting=True,diagnostics=diagnostics)),
                 'Player was not configured',30)
        physical(client,device,'play',diagnostics)
        started=wait_for(lambda:(lambda v:v if v.get('playing') and v['decoded']>=6 and min(v['position'],v['audio_position'])>1 else None)(observe(client,identifier,tab,diagnostics=diagnostics)),
                         'Stall fixture did not start actual media',30)
        report['stage']='stall-hold-'+track+'-'+lane
        policy['gate'].clear()
        wait_for(lambda:policy['blocked_writers']>0,'Stall gate did not block a real writer',15)
        deadline=time.monotonic()+45;stable=0;previous=None;stopped=None
        while time.monotonic()<deadline:
            time.sleep(1)
            current=observe(client,identifier,tab,diagnostics=diagnostics)
            require(not current.get('disposed'),'Stall unexpectedly disposed player')
            require(current['duration']>5 and max(current['position'],current['audio_position'])<current['duration']-5,
                    'Stall fixture reached natural media end')
            if previous is not None and abs(current['position']-previous['position'])<.15 and abs(current['audio_position']-previous['audio_position'])<.15:
                stable+=1
            else:stable=0
            if stable>=3:stopped=current;break
            previous=current
        require(stopped is not None,'Stall did not actually stop both native clocks')
        with fixture.lock:
            held_sent=policy['sent_bytes'];blocked=policy['blocked_writers'];metadata=policy['metadata_bytes']
        require(blocked>0 and held_sent>0,'Stall transport receipt missing')
        report['stage']='stall-release-'+track+'-'+lane
        policy['gate'].set()
        resumed=wait_for(lambda:(lambda v:v if v.get('playing') and v['position']>stopped['position']+1 and v['audio_position']>stopped['audio_position']+1 and v['decoded']>stopped['decoded'] and v['skew']<1 else None)(observe(client,identifier,tab,diagnostics=diagnostics)),
                         'Stall release did not automatically resume media',25)
        before=video_paint(client,device);time.sleep(2);after=video_paint(client,device)
        require(sum(a!=b for a,b in zip(before,after))>30,'Actual OS video paint did not change')
        with fixture.lock:require(fixture.direct==0,'Media bypassed actual selected route')
        physical(client,device,'play',diagnostics)
        require(not observe(client,identifier,tab,diagnostics=diagnostics).get('playing'),'Physical pause failed')
        return {'name':'actual-stall-'+track+'-'+lane,'real_blocked_writers':blocked,
                'actual_start_position':round(started['position'],3),'actual_start_audio_position':round(started['audio_position'],3),
                'held_sent_bytes':held_sent,'metadata_bytes':metadata,'stable_clock_samples':stable,
                'stopped_position':round(stopped['position'],3),'stopped_audio_position':round(stopped['audio_position'],3),
                'automatic_resume_video_advance':round(resumed['position']-stopped['position'],3),
                'automatic_resume_audio_advance':round(resumed['audio_position']-stopped['audio_position'],3),
                'return_skew':resumed['skew'],'return_paint_changed':True,'manual_play_after_release':False,'direct_connections':0}
    finally:fixture.retire_stall(lane,track,policy)


def trusted_fill(client,device,selector,text):
    js_wait(client,'!!document.querySelector('+json.dumps(selector)+')','Native unlock input missing',10)
    client.js('document.querySelector('+json.dumps(selector)+').scrollIntoView({block:"center"});true')
    client.click(selector)
    if sys.platform=='win32':
        import pyautogui
        pyautogui.hotkey('ctrl','a');pyautogui.write(text,interval=.01)
    else:
        env={**os.environ,'DISPLAY':device.display}
        subprocess.run(['xdotool','key','--clearmodifiers','ctrl+a'],env=env,check=True,timeout=5)
        subprocess.run(['xdotool','type','--clearmodifiers','--delay','8','--',text],env=env,check=True,timeout=15)
    js_wait(client,'document.querySelector('+json.dumps(selector)+').value==='+json.dumps(text),
            'Trusted unlock input did not match',5)


def lock_receipt(client,device,fixture,lane,identifier,tab,master,report):
    diagnostics=report['player_diagnostics']
    report['stage']='lock-start-'+lane
    if sys.platform.startswith('linux'):client.js('window.__playerFixtureResults=[];true')
    call(client,'obychnaya_komanda',id=identifier,komanda={'deystvie':'youtube_fixture','id':tab})
    if sys.platform.startswith('linux'):linux_fixture_ack(client,identifier,tab,diagnostics)
    wait_for(lambda:(lambda v:v if v.get('configured') and not v.get('disposed') else None)(observe(client,identifier,tab,admitting=True,diagnostics=diagnostics)),
             'Player was not configured',30)
    physical(client,device,'play',diagnostics)
    started=wait_for(lambda:(lambda v:v if v.get('playing') and v['decoded']>=6 and v['audio_position']>1 else None)(observe(client,identifier,tab,diagnostics=diagnostics)),
                     'Lock fixture did not start actual media',25)
    browser_before=next(row['browser_id'] for row in rows(client,identifier)['vkladki'] if row['id']==tab)
    # No generic Tauri lock command: the CEF view still owns keyboard focus
    # after the single actual Play click.
    client.js('window.__playerStopped=[];true')
    report['stage']='physical-lock-'+lane
    if sys.platform=='win32':
        import pyautogui
        pyautogui.hotkey('ctrl','shift','l')
    else:
        subprocess.run(['xdotool','key','--clearmodifiers','ctrl+shift+l'],env={**os.environ,'DISPLAY':device.display},check=True,timeout=5)
    js_wait(client,'!!document.querySelector(".browser-lock-screen")','Physical shortcut did not show master gate',10)
    # Only the fixed native fixture event remains observable while locked.
    # No generic player command or data IPC is invoked behind the lock gate.
    expression='window.__playerStopped.find(x=>x.super_id==='+json.dumps(identifier)+'&&x.data?.id==='+str(tab)+')'
    receipt=wait_for(lambda:client.js(expression),'Native post-lock stop receipt missing',10)
    data=receipt.get('data',{});state=data.get('state')
    import math
    stop_codes={'stopped','stop_incomplete','stop_observation_timeout','stop_wrong_thread',
                'stop_token_expired','stop_document_changed','stop_token_retired','probe_wrong_thread',
                'probe_timeout','probe_document_changed','probe_message_capacity','probe_observer_unavailable',
                'probe_dispatch_failed','cdp_method_failed','cdp_result_absent','cdp_agent_detached'}
    stop_bools={'disposed','video_paused','audio_paused','video_source_attribute_absent','audio_source_attribute_absent',
                'video_source_object_absent','audio_source_object_absent','video_source_children_absent','audio_source_children_absent'}
    stop_uints={'video_network_state':3,'audio_network_state':3,'video_ready_state':4,'audio_ready_state':4}
    stop_clocks={'position','audio_position'}
    fields=stop_bools|set(stop_uints)|stop_clocks
    safe_state={}
    if isinstance(state,dict):
        for key in stop_bools:
            if type(state.get(key)) is bool:safe_state[key]=state[key]
        for key,maximum in stop_uints.items():
            if type(state.get(key)) is int and 0<=state[key]<=maximum:safe_state[key]=state[key]
        for key in stop_clocks:
            value=state.get(key)
            if type(value) in (int,float) and math.isfinite(value) and 0<=value<=604800:safe_state[key]=value
    observations=data.get('observations');stable_ms=data.get('stable_ms')
    proof_counts_valid=(type(observations) is int and 2<=observations<=4
                        and type(stable_ms) is int and 200<=stable_ms<=5000)
    diagnostics['stop_receipt']={'received':True,
            'complete':data.get('complete') if type(data.get('complete')) is bool else None,
            'code':data.get('code') if isinstance(data.get('code'),str) and data.get('code') in stop_codes else 'unknown_stop_code',
            'state_present':isinstance(state,dict),'state_field_count':len(state) if isinstance(state,dict) else 0,
            'state_valid':isinstance(state,dict) and len(safe_state)==15 and set(state)==fields,
            'state':safe_state,'proof_counts_valid':proof_counts_valid,
            'observations':observations if type(observations) is int and 0<=observations<=4 else None,
            'stable_ms':stable_ms if type(stable_ms) is int and 0<=stable_ms<=5000 else None}
    require(data.get('complete') is True and data.get('code')=='stopped' and isinstance(state,dict),
            'Native post-lock stop receipt incomplete')
    require(set(state)==fields and all(state[name] is True for name in stop_bools),
            'Native post-lock media not actually stopped')
    require(all(type(state[name]) is int and state[name]==0 for name in stop_uints),
            'Native post-lock resource not actually detached')
    require(all(type(state[name]) in (int,float) and math.isfinite(state[name]) and 0<=state[name]<=604800 for name in stop_clocks),
            'Native post-lock clocks invalid')
    require(proof_counts_valid,'Native post-lock stable observations missing')
    report['stage']='physical-unlock-'+lane
    foreground(device)
    trusted_fill(client,device,'[aria-label="Мастер-пароль для разблокировки браузера"]',master)
    client.text_button('Разблокировать браузер')
    js_wait(client,'!document.querySelector(".browser-lock-screen")&&!!document.querySelector(".okno")',
            'Actual master UI did not unlock browser',25)
    require(next(row['browser_id'] for row in rows(client,identifier)['vkladki'] if row['id']==tab)==browser_before,
            'Lock unexpectedly replaced the actual player browser')
    # Old authority must remain revoked after unlocking. This is distinct from
    # an explicit, new fixture admission below (no claim about production CtrlR).
    client.js('window.__playerObserved=[];true')
    call(client,'obychnaya_komanda',id=identifier,komanda={'deystvie':'youtube_snapshot','id':tab})
    old=wait_for(lambda:client.js('window.__playerObserved.find(x=>x.super_id==='+json.dumps(identifier)+'&&x.data?.id==='+str(tab)+')'),
                 'Old player authority refusal missing',10)
    require('state' not in old.get('data',{}) and isinstance(old.get('data',{}).get('error'),str),
            'Old player authority survived browser lock')
    report['stage']='new-fixture-admission-after-unlock-'+lane
    if sys.platform.startswith('linux'):client.js('window.__playerFixtureResults=[];true')
    call(client,'obychnaya_komanda',id=identifier,komanda={'deystvie':'youtube_fixture','id':tab})
    if sys.platform.startswith('linux'):linux_fixture_ack(client,identifier,tab,diagnostics)
    paused=wait_for(lambda:(lambda v:v if v.get('configured') and not v.get('disposed') else None)(observe(client,identifier,tab,admitting=True,diagnostics=diagnostics)),
                    'Player was not configured',30)
    require(not paused.get('playing'),'New fixture started without explicit Play')
    time.sleep(1)
    still=observe(client,identifier,tab,diagnostics=diagnostics)
    require(not still.get('playing') and abs(still['audio_position']-paused['audio_position'])<.15,
            'New fixture did not remain paused')
    physical(client,device,'play',diagnostics)
    resumed=wait_for(lambda:(lambda v:v if v.get('playing') and v['decoded']>=6 and v['audio_position']>1 else None)(observe(client,identifier,tab,diagnostics=diagnostics)),
                     'Explicit Play after unlock did not resume new fixture',25)
    with fixture.lock:require(fixture.direct==0,'Media bypassed actual selected route')
    physical(client,device,'play',diagnostics)
    require(not observe(client,identifier,tab,diagnostics=diagnostics).get('playing'),'Physical pause failed')
    return {'name':'native-lock-stops-both-media-'+lane,'physical_lock':True,'native_stop_complete':True,
            'resources_detached':True,'native_stop_state':safe_state,
            'stop_observations':observations,'stop_stable_ms':stable_ms,
            'stop_position':state['position'],'stop_audio_position':state['audio_position'],
            'actual_master_ui_unlock':True,'same_native_browser_after_unlock':True,'old_grant_refused':True,'new_admission_initially_paused':True,
            'explicit_play_decoded':resumed['decoded'],'direct_connections':0,'production_reload_tested':False}


class ExitWitness:
    """Bounded native-owned identities + OS births, not a universal escape proof."""
    def __init__(self,device,client):
        import psutil
        self.psutil=psutil;self.clock='windows_filetime' if sys.platform=='win32' else 'linux_ticks'
        require(device.app.poll() is None,'Privacy parent already exited')
        self.identities={device.app.pid:kernel_birth(device.app.pid,self.clock)};self.roots={device.app.pid};self.lock=threading.Lock()
        self.done=threading.Event();self.error=False
        native=call(client,'super_lifecycle_snapshot')
        require(native.get('schema')==1 and isinstance(native.get('engines'),list),'Privacy native ownership schema invalid')
        members=[]
        for row in native['engines']:
            require(row.get('owned_complete') is True and isinstance(row.get('owned'),list),'Privacy native ownership incomplete')
            if row.get('pid'):self.roots.add(row['pid'])
            for member in row['owned']:
                require(isinstance(member,dict) and type(member.get('pid')) is int and member['pid']>0
                        and member.get('clock')==self.clock and isinstance(member.get('birth'),str)
                        and member['birth'].isdigit(),'Privacy native process identity invalid')
                members.append(member)
        require(members,'Privacy native process members missing')
        for member in members:
            try:actual=kernel_birth(member['pid'],self.clock)
            except psutil.NoSuchProcess:continue
            require(actual==member['birth'],'Privacy native process identity changed')
            self.identities[member['pid']]=member['birth']
        self.native_members=len(members)
        self.sample()
        self.thread=threading.Thread(target=self.run,daemon=True);self.thread.start()
    def sample(self):
        for pid in self.roots:
            try:
                expected=self.identities.get(pid)
                if expected is not None and kernel_birth(pid,self.clock)!=expected:continue
                root=self.psutil.Process(pid)
                for process in [root]+root.children(recursive=True):
                    try:
                        birth=kernel_birth(process.pid,self.clock)
                        with self.lock:
                            old=self.identities.get(process.pid)
                            if old is not None and old!=birth:raise RuntimeError('identity_reused')
                            self.identities[process.pid]=birth
                    except self.psutil.NoSuchProcess:pass
            except self.psutil.NoSuchProcess:pass
    def run(self):
        while not self.done.is_set():
            try:self.sample()
            except Exception:self.error=True;return
            self.done.wait(.02)
    def all_gone(self):
        require(not self.error,'Privacy process observation incomplete')
        with self.lock:rows=list(self.identities.items())
        for pid,birth in rows:
            try:
                process=self.psutil.Process(pid)
                if not process.is_running() or process.status()==self.psutil.STATUS_ZOMBIE:continue
                if kernel_birth(pid,self.clock)==birth:return False
            except self.psutil.NoSuchProcess:pass
        return True
    def close(self):
        self.done.set();self.thread.join(timeout=3)
        require(not self.thread.is_alive() and not self.error,'Privacy process observation incomplete')


def privacy_canary(client,device,fixture,lane,identifier,report):
    report['stage']='privacy-cache-canary-'+lane
    foreground(device)
    click_control(client,'.obychnaya[data-id="2"]',15)
    canary='https://'+HOST+'/cache-canary'
    call(client,'obychnaya_komanda',id=identifier,komanda={'deystvie':'pereyti','id':2,'adres':canary})
    def loaded():
        with fixture.lock:seen=lane in fixture.canary_receipts
        native=next(row for row in rows(client,identifier)['vkladki'] if row['id']==2)
        return seen and native.get('browser_id',0)>0 and not native.get('zagruzka') and native.get('adres')==canary
    wait_for(loaded,'Cacheable control did not actually load through selected route',25)
    painted(client)
    def actual_canary_paint():
        pixels=video_paint(client,device)
        return sum(tuple(pixels[i:i+3])==(28,112,80) for i in range(0,len(pixels),3))>=500
    wait_for(actual_canary_paint,'Cacheable control did not actually paint',15)
    time.sleep(1)
    with fixture.lock:
        require(fixture.direct==0,'Media bypassed actual selected route')
        media=[row for row in fixture.receipts if row['lane']==lane]
        require(media and all(row.get('privacy_marked') is True for row in media)
                and {row['track'] for row in media}=={'video','audio'},
                'Native media privacy sentinel was not requested')
    return {'name':'cacheable-body-control-'+lane,'actual_routed_response':True,'native_loaded':True,'actual_os_paint':True,
            'media_sentinel_requested':True,'marker_in_control_url':False,'direct_connections':0}



def parse_shutdown_receipt(value,parent_pid):
    keys={'schema','prepared_update','parent_pid','owners','clean','forced','unacknowledged','unclean','pending_starts','complete'}
    require(isinstance(value,dict) and set(value)==keys,'Shutdown receipt invalid')
    integers=('schema','parent_pid','owners','clean','forced','unacknowledged','unclean','pending_starts')
    require(all(type(value[key]) is int for key in integers)
            and type(value['prepared_update']) is bool and type(value['complete']) is bool,
            'Shutdown receipt invalid')
    require(value['schema']==1 and value['parent_pid']==parent_pid
            and 1<=value['owners']<=1024
            and all(0<=value[key]<=1024 for key in ('clean','forced','unacknowledged','unclean','pending_starts')),
            'Shutdown receipt invalid')
    require(value['complete'] is True and value['clean']==value['owners']
            and all(value[key]==0 for key in ('forced','unacknowledged','unclean','pending_starts')),
            'Shutdown receipt unclean')
    return value


def read_shutdown_receipt(data,parent_pid):
    """Only after the held parent and all known native executions have exited."""
    root=pathlib.Path(data);path=root/'app-shutdown-receipt.json'
    def ordinary(metadata):
        return not stat.S_ISLNK(metadata.st_mode) and not getattr(metadata,'st_file_attributes',0)&0x400
    def identity(metadata):return metadata.st_dev,metadata.st_ino
    def unique(pairs):
        result={}
        for key,value in pairs:
            if key in result:raise ValueError('duplicate')
            result[key]=value
        return result
    try:
        before_root=os.lstat(root)
        require(stat.S_ISDIR(before_root.st_mode) and ordinary(before_root),'Shutdown receipt invalid')
        before=os.lstat(path)
        require(stat.S_ISREG(before.st_mode) and ordinary(before) and before.st_nlink==1
                and 0<before.st_size<=4096,'Shutdown receipt invalid')
        flags=os.O_RDONLY|getattr(os,'O_NOFOLLOW',0)|getattr(os,'O_BINARY',0)
        with os.fdopen(_open_readonly(path,flags),'rb') as stream:
            opened=os.fstat(stream.fileno());native=_windows_info(stream.fileno())
            require(stat.S_ISREG(opened.st_mode) and ordinary(opened) and opened.st_nlink==1
                    and identity(opened)==identity(before) and opened.st_size==before.st_size,
                    'Shutdown receipt invalid')
            require(native is None or not native['reparse'] and native['number_of_links']==1,
                    'Shutdown receipt invalid')
            contents=stream.read(4097);after=os.fstat(stream.fileno());native_after=_windows_info(stream.fileno())
            require(0<len(contents)<=4096 and len(contents)==before.st_size
                    and identity(after)==identity(opened) and after.st_size==opened.st_size
                    and after.st_mtime_ns==opened.st_mtime_ns and ordinary(after) and after.st_nlink==1,
                    'Shutdown receipt invalid')
            require(native_after is None or native_after==native,'Shutdown receipt invalid')
        final=os.lstat(path);final_root=os.lstat(root)
        require(identity(final)==identity(before) and ordinary(final) and stat.S_ISREG(final.st_mode)
                and final.st_nlink==1 and final.st_size==before.st_size
                and identity(final_root)==identity(before_root) and ordinary(final_root),
                'Shutdown receipt invalid')
        return parse_shutdown_receipt(json.loads(contents.decode('utf-8'),object_pairs_hook=unique),parent_pid)
    except FileNotFoundError:
        require(False,'Shutdown receipt missing')
    except (OSError,ValueError,UnicodeError):
        require(False,'Shutdown receipt invalid')


def shutdown_receipt_self_test():
    valid=dict(schema=1,prepared_update=False,parent_pid=12345,owners=2,clean=2,forced=0,
               unacknowledged=0,unclean=0,pending_starts=0,complete=True)
    def refused(value,pid=12345):
        try:parse_shutdown_receipt(value,pid)
        except Exception:return
        raise AssertionError('Unproven shutdown accepted')
    assert parse_shutdown_receipt(valid,12345)==valid
    refused(valid,54321)
    for key,value in [('schema',2),('owners',0),('owners',True),('clean',1),('forced',1),
                      ('unacknowledged',1),('unclean',1),('pending_starts',1),('complete',False)]:
        refused(dict(valid,**{key:value}))
    refused(dict(valid,extra='private'))
    with tempfile.TemporaryDirectory(prefix='nb-shutdown-proof-') as directory:
        root=pathlib.Path(directory);path=root/'app-shutdown-receipt.json'
        path.write_text(json.dumps(valid),encoding='utf-8')
        assert read_shutdown_receipt(root,12345)==valid
        def file_refused():
            try:read_shutdown_receipt(root,12345)
            except Exception:return
            raise AssertionError('Unsafe receipt file accepted')
        path.write_text(json.dumps(valid)[:-1]+',"owners":2}',encoding='utf-8');file_refused()
        path.write_bytes(b'x'*4097);file_refused()
        path.unlink();file_refused()
        target=root/'other';target.write_text(json.dumps(valid),encoding='utf-8')
        try:path.symlink_to(target)
        except OSError:pass
        else:file_refused();path.unlink()
        os.link(target,path);file_refused();path.unlink()
    return {'self_test':True,'synthetic_only':True,'native_execution':False}


def close_for_privacy(client,device,report):
    report['stage']='privacy-normal-owned-exit'
    identity=native_app_identity(device)
    witness=ExitWitness(device,client)
    parent_pid=identity['pid']  # Receipt names NexusBro, not a dbus-run-session launcher.
    with witness.lock:witness.identities[parent_pid]=identity['birth']
    try:
        require(native_app_identity_alive(device,identity),'Native app identity invalid')
        foreground(device)
        require(native_app_identity_alive(device,identity),'Native app identity invalid')
        owner=native_app_window(device,identity)
        if sys.platform=='win32':owner.close()
        else:
            # Ask the owned WM for normal window close, like its titlebar X.
            # A focused CEF child may intercept Alt+F4; never kill/destroy it.
            subprocess.run(['wmctrl','-ic',str(owner)],
                           env=device.player_wm.env,check=True,timeout=5)
        exit_code=device.app.wait(timeout=30)
        require(exit_code==0,'Privacy parent did not exit normally')
        wait_for(witness.all_gone,'Privacy owned native processes did not exit',30)
        witness.close()
        require(witness.all_gone(),'Privacy owned native processes did not exit')
        require(not native_app_execution_alive(identity),'Native app execution remained alive')
        report['owned_exit']={'parent_normal_exit':True,'native_members_observed':witness.native_members,
                              'os_identities_observed':len(witness.identities),'all_known_native_executions_exited':True,
                              'native_members_complete':True,'tracker_seconds':.02,
                              'scope':'bounded_owned_fixture_not_universal_noescape_proof','harness_kill_used_for_acceptance':False,
                              'clean_shutdown_receipt_verified':False,'actual_app_identity_bound':True,
                              'launcher_is_actual_app':identity['pid']==identity['wrapper_pid'],
                              'actual_app_execution_exited':True,'launcher_exit_zero':True}
        # Prevent generic emergency cleanup from signalling an already reaped
        # PID/PGID that could later be reused while the disk scan runs.
        device.app=None
        report['app_shutdown_receipt']=read_shutdown_receipt(device.data,parent_pid)
        report['owned_exit']['clean_shutdown_receipt_verified']=True
    finally:
        witness.close()


def run(args):
    require(os.environ.get('GITHUB_ACTIONS')=='true','Disposable GHA runner required')
    out=pathlib.Path(args.out).resolve();out.mkdir(parents=True,exist_ok=False)
    report={'schema':1,'completed':False,'extractor_acceptance':False,'checks':[],'stage':'setup','player_diagnostics':{}}
    code=1;device=fixture=trust=wm=None;private_root=None
    old=os.environ.get('NEXUSBRO_TEST_YOUTUBE_DNS_GUARD')
    old_sentinel=os.environ.get('NEXUSBRO_TEST_YOUTUBE_MEDIA_SENTINEL')
    try:
        private_root=pathlib.Path(tempfile.mkdtemp(prefix='nb-player-'))
        original_root=private_root
        private_root=private_root.resolve(strict=True)
        report['privacy_sandbox']={'canonical_equal':private_root.resolve(strict=True)==private_root,
                                   'original_diff':original_root!=private_root}
        if private_root:
            root=private_root;thumb=certificates(root/'tls')
            trust=Trust(root,root/'tls'/'ca.crt',thumb);trust.install()
            media_sentinel=secrets.token_hex(24);positive_sentinel=secrets.token_hex(24)
            fixture=Fixture(root/'tls'/'server.crt',root/'tls'/'server.key',pathlib.Path(args.video_webm),pathlib.Path(args.audio_webm),media_sentinel,positive_sentinel)
            report['tls_handshake_preflight']=tls_proxy_preflight(fixture.proxies['A'],HOST,443,root/'tls'/'ca.crt',root/'tls'/'server.crt')
            exe,cef=pathlib.Path(args.exe).resolve(),pathlib.Path(args.cef).resolve()
            device=WindowsDevice(root/'device',exe,cef) if sys.platform=='win32' else Device('player',root/'device',exe,cef)
            device.player_sidebar_out=out
            device.player_sidebar_testdata=device.data.resolve()
            definitions=[]
            for lane in ('A','B'):
                identifier='player-'+lane.lower();item=super_record(identifier,'Player '+lane,'about:blank',proxy='http://127.0.0.1:'+str(fixture.proxies[lane]));item['poryadok']=len(definitions);definitions.append(item)
                folder=device.data/'profili'/identifier;folder.mkdir(parents=True)
                (folder/'obychnye.json').write_text(json.dumps({'vkladki':[{'id':1,'adres':'about:blank','zagolovok':'Fixture','otlozhena':True},{'id':2,'adres':'about:blank','zagolovok':'Neighbor','otlozhena':True}],'aktivnaya':1,'sleduyushchaya':3}))
            for filename in ('vkladki.json','sohranennye.json'):(device.data/filename).write_text(json.dumps(definitions))
            (device.data/'nastrojki.json').write_text(json.dumps({'avtoobnovleniya':False,'apparatnoe_uskorenie':False}))
            os.environ['NEXUSBRO_TEST_YOUTUBE_DNS_GUARD']='1'
            os.environ['NEXUSBRO_TEST_YOUTUBE_MEDIA_SENTINEL']=media_sentinel
            master='nb-player-'+secrets.token_hex(16)
            client=device.start()
            if sys.platform.startswith('linux'):
                wm=OwnedWindowManager(device,root/'wm');device.player_wm=wm
            foreground(device)
            require(call(client,'security_vault',action='create',password=master).get('unlocked') is True,'Synthetic fixture vault not unlocked')
            client.js('window.__playerObserved=[];window.__cefFocusTrace=[];window.__playerListenerReady=false;window.__playerHandler=window.__TAURI_INTERNALS__.transformCallback(e=>{if(e.payload?.data?.tip==="youtube-state")window.__playerObserved.push(e.payload);if(e.payload?.data?.tip==="cef-focus-trace"){window.__cefFocusTrace.push(e.payload);if(window.__cefFocusTrace.length>128)window.__cefFocusTrace.shift()}});window.__TAURI_INTERNALS__.invoke("plugin:event|listen",{event:"stranica-ui",target:{kind:"Window",label:"main"},handler:window.__playerHandler}).then(()=>window.__playerListenerReady=true);true')
            js_wait(client,'window.__playerListenerReady','Actual native event listener missing')
            client.js('window.__playerStopped=[];window.__playerStopListenerReady=false;window.__playerStopHandler=window.__TAURI_INTERNALS__.transformCallback(e=>{if(e.payload?.data?.tip==="youtube-stop-state")window.__playerStopped.push(e.payload)});window.__TAURI_INTERNALS__.invoke("plugin:event|listen",{event:"stranica-ui",target:{kind:"Window",label:"main"},handler:window.__playerStopHandler}).then(()=>window.__playerStopListenerReady=true);true')
            js_wait(client,'window.__playerStopListenerReady','Native stop listener missing')
            if sys.platform.startswith('linux'):
                client.js('window.__playerFixtureResults=[];window.__playerFixtureListenerReady=false;window.__playerFixtureHandler=window.__TAURI_INTERNALS__.transformCallback(e=>{if(e.payload?.data?.tip==="youtube-result")window.__playerFixtureResults.push(e.payload)});window.__TAURI_INTERNALS__.invoke("plugin:event|listen",{event:"stranica-ui",target:{kind:"Window",label:"main"},handler:window.__playerFixtureHandler}).then(()=>window.__playerFixtureListenerReady=true);true')
                js_wait(client,'window.__playerFixtureListenerReady','Actual native fixture listener missing')
            for lane in ('A','B'):
                identifier='player-'+lane.lower();report['stage']='load-'+lane
                foreground(device);switch_super(client,identifier)
                if client.js('!!document.querySelector(".pusto button")'):click_control(client,'.pusto button',20)
                session=wait_for(lambda:rows(client,identifier) if any(r.get('browser_id',0)>0 and not r.get('zagruzka') for r in rows(client,identifier)['vkladki']) else None,'Actual native browser not loaded',40)
                tab=session['aktivnaya']
                if sys.platform.startswith('linux'):client.js('window.__playerFixtureResults=[];true')
                call(client,'obychnaya_komanda',id=identifier,komanda={'deystvie':'youtube_fixture','id':tab})
                if sys.platform.startswith('linux'):linux_fixture_ack(client,identifier,tab,report['player_diagnostics'])
                initial=wait_for(lambda:(lambda s:s if s.get('configured') and not s.get('disposed') else None)(observe(client,identifier,tab,admitting=True,diagnostics=report['player_diagnostics'])),'Player was not configured',30)
                report['stage']='physical-play-'+lane;physical(client,device,'play',report['player_diagnostics'])
                first=wait_for(lambda:(lambda s:s if s.get('playing') and s.get('decoded',0)>=3 else None)(observe(client,identifier,tab,diagnostics=report['player_diagnostics'])),'Physical play did not decode actual media',25)
                require(first['video_width']==320 and first['video_height']==180 and first['separate'],'Actual VP9/Opus geometry differs')
                paint_before=video_paint(client,device)
                time.sleep(3);second=observe(client,identifier,tab,diagnostics=report['player_diagnostics'])
                paint_after=video_paint(client,device)
                require(sum(a!=b for a,b in zip(paint_before,paint_after))>30,'Actual OS video paint did not change')
                require(second['position']>first['position']+1 and second['audio_position']>first['audio_position']+1 and second['skew']<1,'Dual media clocks failed')
                report['stage']='physical-pause-'+lane;physical(client,device,'play',report['player_diagnostics']);time.sleep(.5);paused=observe(client,identifier,tab,diagnostics=report['player_diagnostics']);time.sleep(1);still=observe(client,identifier,tab,diagnostics=report['player_diagnostics'])
                require(not paused['playing'] and abs(still['position']-paused['position'])<.35,'Physical pause failed')
                report['stage']='physical-seek-'+lane;physical(client,device,'seek',report['player_diagnostics']);time.sleep(1);seek=observe(client,identifier,tab,diagnostics=report['player_diagnostics'])
                require(abs(seek['position']-still['position'])>5,'Physical seek did not move actual native media')
                with fixture.lock:
                    tracks={r['track'] for r in fixture.receipts if r['lane']==lane};direct=fixture.direct
                    range_count=sum(r['range'] for r in fixture.receipts if r['lane']==lane)
                require(tracks=={'video','audio'} and direct==0,'Media bypassed actual selected route')
                report['stage']='background-new-fixture-'+lane
                browser_before=next(row['browser_id'] for row in rows(client,identifier)['vkladki'] if row['id']==tab)
                setup={'lane':lane,'explicit_new_fixture_admission':False,'same_native_browser':False,
                       'initial_paused':False,'home_keyboard_tested':False,
                       'baseline_physical_seek_delta':round(abs(seek['position']-still['position']),3)}
                report['player_diagnostics'].setdefault('background_setup',[]).append(setup)
                # Baseline pause/physical seek remains independently checked.
                # A fresh actual document provides a full 120-second background
                # budget without claiming Linux native range Home support.
                if sys.platform.startswith('linux'):client.js('window.__playerFixtureResults=[];true')
                call(client,'obychnaya_komanda',id=identifier,komanda={'deystvie':'youtube_fixture','id':tab})
                if sys.platform.startswith('linux'):linux_fixture_ack(client,identifier,tab,report['player_diagnostics'])
                setup['explicit_new_fixture_admission']=True
                fresh=wait_for(lambda:(lambda state:state if state.get('configured') and not state.get('disposed')
                        and not state.get('playing') and state['position']<2 and state['audio_position']<2 else None)
                        (observe(client,identifier,tab,admitting=True,diagnostics=report['player_diagnostics'])),
                        'Fresh background fixture did not start paused at the beginning',30)
                setup.update(initial_paused=True,initial_position=fresh['position'],initial_audio_position=fresh['audio_position'])
                setup['same_native_browser']=next(row['browser_id'] for row in rows(client,identifier)['vkladki'] if row['id']==tab)==browser_before
                require(setup['same_native_browser'],'Fresh background fixture replaced native browser')
                report['stage']='background-play-'+lane
                physical(client,device,'play',report['player_diagnostics'])
                played=wait_for(lambda:(lambda state:state if state.get('playing') and state['decoded']>=3
                        and state['audio_position']>fresh['audio_position']+.5 and state['position']>fresh['position']+.5 else None)
                        (observe(client,identifier,tab,diagnostics=report['player_diagnostics'])),
                        'Background test requires actual playback',25)
                setup.update(explicit_play_count=1,decoded=played['decoded'],audio_advance=played['audio_position']-fresh['audio_position'])
                report['stage']='background-tab-'+lane
                report['checks'].append(background_receipt(client,device,identifier,tab,
                    lambda:click_control(client,'.obychnaya[data-id="2"]',15),
                    lambda:click_control(client,'.obychnaya[data-id="'+str(tab)+'"]',15),
                    'background-ordinary-'+lane,report['player_diagnostics']))
                other='player-b' if lane=='A' else 'player-a'
                report['stage']='background-super-'+lane
                report['checks'].append(background_receipt(client,device,identifier,tab,
                    lambda:switch_super_foreground(client,device,other),lambda:switch_super_foreground(client,device,identifier),
                    'background-super-'+lane,report['player_diagnostics']))
                report['stage']='actual-minimize-'+lane
                report['checks'].append(background_receipt(client,device,identifier,tab,
                    lambda:minimize_actual(device,True),lambda:minimize_actual(device,False),
                    'native-minimize-'+lane,report['player_diagnostics']))
                physical(client,device,'play',report['player_diagnostics'])
                require(not observe(client,identifier,tab,diagnostics=report['player_diagnostics']).get('playing'),'Physical pause failed')
                report['checks'].append({'name':'actual-dual-webm-'+lane,'decoded':second['decoded'],'width':second['video_width'],'height':second['video_height'],'skew':second['skew'],'range_requests':range_count,'physical_pause':True,'physical_seek':True,'actual_os_paint_changed':True,'direct_connections':direct})
                for stalled_track in ('video','audio'):
                    report['checks'].append(stall_receipt(client,device,fixture,lane,identifier,tab,stalled_track,report))
                report['checks'].append(lock_receipt(client,device,fixture,lane,identifier,tab,master,report))
                report['checks'].append(privacy_canary(client,device,fixture,lane,identifier,report))
            close_for_privacy(client,device,report)
            report['stage']='privacy-disk-scan'
            profile_root=device.data/'profili'
            require(profile_root.is_dir() and all((profile_root/('player-'+lane.lower())).is_dir() for lane in ('A','B')),
                    'Privacy actual CEF profile missing')
            try:
                scans=[{'lane':lane,**scan_profile(root,profile_root/('player-'+lane.lower()),media_sentinel,positive_sentinel,
                        max_files=4096,max_bytes=256*1024*1024,max_seconds=15)} for lane in ('A','B')]
            except ValueError as error:
                allowed={'privacy_marker_invalid','privacy_markers_overlap','privacy_scan_bounds_invalid',
                         'privacy_scan_not_disposable','privacy_scan_scope_invalid'}
                code=str(error)
                report['privacy_scan_error']=code if code in allowed else 'privacy_scan_unknown_error'
                report['privacy_sandbox']['canonical_equal']=root.resolve(strict=True)==root
                raise
            report['disk_privacy']={'schema':1,'complete':all(row['complete'] for row in scans),
                    'accepted':all(row['accepted'] for row in scans),'profiles':scans,
                    'media_files':sum(row['media_files'] for row in scans),
                    'positive_cache_files':sum(row['positive_cache_files'] for row in scans),
                    'files':sum(row['files'] for row in scans),'bytes':sum(row['bytes'] for row in scans)}
            require(report['disk_privacy']['complete'] and report['disk_privacy']['accepted'],
                    'Actual cache privacy scan not accepted')
            report['completed']=True;report['stage']='complete';code=0
    except Exception as error:
        # Code locations only: never serialize exception messages, paths or locals.
        frames=[]
        trace=error.__traceback__
        while trace:
            name=trace.tb_frame.f_code.co_name
            if name in {'run','background_receipt','switch_super','click_control','js_wait','wait_for','require','observe','row_record'}:
                frames.append({'function':name,'line':trace.tb_lineno})
            trace=trace.tb_next
        report['failure_trace']=frames[-8:]
        kind=type(error).__name__
        report['failure_class']=kind if kind in {'NativeFailure','AssertionError','TimeoutError','RuntimeError','ValueError','KeyError','StopIteration'} else 'other'
        codes={'Native fixture admission acknowledgment missing':'fixture_ack_missing',
               'Native fixture admission refused':'fixture_admission_refused',
               'Actual native fixture listener missing':'fixture_listener_missing',
               'Native player controls not physically painted':'controls_not_painted',
               'Physical play did not decode actual media':'playback_not_decoding',
               'Native player observation missing':'snapshot_missing','Native observation refused':'snapshot_refused',
               'Player was not configured':'not_configured','Dual media clocks failed':'clocks_not_advancing',
               'Physical pause failed':'pause_failed','Physical seek did not move actual native media':'seek_failed',
               'Actual VP9/Opus geometry differs':'media_geometry','Actual OS video paint did not change':'video_paint_static',
               'Media bypassed actual selected route':'route_witness_missing','Native player viewport too small':'viewport_too_small',
               'Owned minimize window manager unavailable':'window_manager_missing',
               'Owned minimize window manager not ready':'window_manager_not_ready',
               'Owned minimize window manager exited':'window_manager_exited',
               'Native minimize owner ambiguous':'minimize_owner_ambiguous',
               'Native owner parent identity changed':'native_owner_parent_changed',
               'Native owner observation outside bounds':'native_owner_bound',
               'Native minimize receipt missing':'minimize_receipt_missing',
               'Background audio froze or paused':'background_audio_freeze',
               'Background diagnostic case bound exceeded':'background_diagnostic_case_bound',
               'Background diagnostic snapshot bound exceeded':'background_diagnostic_snapshot_bound',
               'Background return did not automatically resume synchronized decode':'background_return_failed',
               'Background playback browser changed':'background_browser_changed',
               'Background test requires actual playback':'background_not_playing',
               'Physical rewind failed':'rewind_failed',
               'Fresh background fixture did not start paused at the beginning':'background_fresh_not_paused',
               'Fresh background fixture replaced native browser':'background_fresh_browser_changed',
               'Privacy native ownership schema invalid':'privacy_ownership_schema',
               'Privacy native ownership incomplete':'privacy_ownership_incomplete',
               'Privacy native process identity invalid':'privacy_identity_invalid',
               'Privacy native process members missing':'privacy_members_missing',
               'Privacy native process identity changed':'privacy_identity_changed',
               'Privacy process observation incomplete':'privacy_observation_incomplete',
               'Cacheable control did not actually load through selected route':'privacy_canary_not_loaded',
               'Cacheable control did not actually paint':'privacy_canary_not_painted',
               'Native media privacy sentinel was not requested':'privacy_sentinel_not_requested',
               'Native app identity invalid':'native_app_identity_invalid',
               'Native app execution remained alive':'native_app_execution_alive',
               'Shutdown receipt missing':'shutdown_receipt_missing',
               'Shutdown receipt invalid':'shutdown_receipt_invalid',
               'Shutdown receipt unclean':'shutdown_receipt_unclean',
               'Privacy parent already exited':'privacy_parent_already_exited',
               'Privacy parent did not exit normally':'privacy_parent_exit_failed',
               'Privacy owned native processes did not exit':'privacy_owned_exit_failed',
               'Privacy actual CEF profile missing':'privacy_profile_missing',
               'Actual cache privacy scan not accepted':'privacy_scan_not_accepted',
               **{code:code for code in ('privacy_marker_invalid','privacy_markers_overlap','privacy_scan_bounds_invalid',
                                         'privacy_scan_not_disposable','privacy_scan_scope_invalid')},
               'Synthetic fixture vault not unlocked':'fixture_vault_failed',
               'Native stop listener missing':'stop_listener_missing',
               'Lock fixture did not start actual media':'lock_start_failed',
               'Physical shortcut did not show master gate':'physical_lock_failed',
               'Native post-lock stop receipt missing':'stop_receipt_missing',
               'Native post-lock stop receipt incomplete':'stop_receipt_incomplete',
               'Native post-lock media not actually stopped':'stop_media_not_stopped',
               'Native post-lock clocks invalid':'stop_clocks_invalid',
               'Native post-lock resource not actually detached':'stop_resource_attached',
               'Native post-lock stable observations missing':'stop_stable_observations_missing',
               'Native unlock input missing':'unlock_input_missing',
               'Trusted unlock input did not match':'unlock_input_mismatch',
               'Actual master UI did not unlock browser':'master_unlock_failed',
               'Lock unexpectedly replaced the actual player browser':'lock_browser_changed',
               'Old player authority refusal missing':'old_grant_refusal_missing',
               'Old player authority survived browser lock':'old_grant_survived',
               'New fixture started without explicit Play':'new_fixture_autoplay',
               'New fixture did not remain paused':'new_fixture_pause_failed',
               'Explicit Play after unlock did not resume new fixture':'new_fixture_play_failed',
               'Stall fixture media too small for bounded metadata bypass':'stall_media_too_small',
               'Stall fixture did not start actual media':'stall_start_failed',
               'Stall gate did not block a real writer':'stall_writer_not_blocked',
               'Stall unexpectedly disposed player':'stall_disposed',
               'Stall fixture reached natural media end':'stall_natural_end',
               'Stall did not actually stop both native clocks':'stall_clocks_not_stopped',
               'Stall transport receipt missing':'stall_transport_missing',
               'Stall release did not automatically resume media':'stall_auto_resume_failed',
               'windows_fixture_chain_not_trusted':'windows_chain_untrusted',
               'windows_chain_operation_failed':'windows_chain_operation_failed'}
        report['failure_code']=codes.get(str(error),'unclassified')
    finally:
        if device and getattr(device,'client',None):
            try:report['cef_focus_receipts']=focus_trace_receipt(device.client)
            except Exception:report['cef_focus_receipt_unavailable']=True
        if device and getattr(device,'player_owner_diagnostics',None):report['native_owner']=device.player_owner_diagnostics
        if trust and trust.windows:report['windows_chain_preflight']=trust.windows.receipts
        cleanup_ok=True
        for owned in (wm,device,fixture,trust):
            if owned:
                try:owned.close()
                except Exception:cleanup_ok=False
        report['cleanup_completed']=cleanup_ok
        if not cleanup_ok:report['completed']=False;code=1
        if private_root:shutil.rmtree(private_root,ignore_errors=True)
        if old is None:os.environ.pop('NEXUSBRO_TEST_YOUTUBE_DNS_GUARD',None)
        else:os.environ['NEXUSBRO_TEST_YOUTUBE_DNS_GUARD']=old
        if old_sentinel is None:os.environ.pop('NEXUSBRO_TEST_YOUTUBE_MEDIA_SENTINEL',None)
        else:os.environ['NEXUSBRO_TEST_YOUTUBE_MEDIA_SENTINEL']=old_sentinel
        (out/'result.json').write_text(json.dumps(report,indent=2)+'\n')
    return code


if __name__=='__main__':
    if sys.argv[1:]==['--self-test-native-app-identity']:print(json.dumps(native_app_identity_self_test()));raise SystemExit(0)
    if sys.argv[1:]==['--self-test-shutdown-receipt']:print(json.dumps(shutdown_receipt_self_test()));raise SystemExit(0)
    if sys.argv[1:]==['--self-test-restore-observer']:print(json.dumps(restore_observer_self_test()));raise SystemExit(0)
    if sys.argv[1:]==['--self-test-sidebar']:print(json.dumps(sidebar_self_test()));raise SystemExit(0)
    parser=argparse.ArgumentParser(description=__doc__)
    for name in ('exe','cef','out','video-webm','audio-webm'):parser.add_argument('--'+name,required=True)
    sys.exit(run(parser.parse_args()))
