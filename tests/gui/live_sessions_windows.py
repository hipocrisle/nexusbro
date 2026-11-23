"""Read-only native session observer for disposable Windows acceptance runners.

Portable recovery JSON deliberately contains no live lifecycle state. Physical
UIA still drives every action; this observer only reads real shell IPC replies.
No background poll, application patches, CEF inspection or substituted data.
"""
import json
import os
import pathlib
import sys
import time

import websocket
from security_native import port
from tab_routes import OwnedInspectorPolicy, WindowsClient, windows_token_elevated


class LiveSessions:
    def __init__(self, executable):
        if sys.platform!='win32' or os.environ.get('GITHUB_ACTIONS')!='true':
            raise RuntimeError('Live inspector observer is limited to disposable Windows CI')
        self.executable=pathlib.Path(executable).resolve()
        self.port=port();self.policy=None;self.client=None;self.app=None
        self.diagnostics={'source':'actual_readonly_Tauri_IPC','window':'main',
            'runtime_session_command':'obychnye_spisok','engine_membership_command':'spisok',
            'python_disk_runtime_fallback':False,'background_poll':False,
            'ipc_requests':0,'live_sessions':0,'dormant_snapshots':0,
            'transient_unavailable':0,'attach_attempts':0,'last_stage':'not_started'}

    def configure(self, env):
        """Must run before spawning the owned application; restore in finally."""
        self.diagnostics['last_stage']='owned_policy_setup'
        self.policy=OwnedInspectorPolicy(self.executable.name,self.port,windows_token_elevated())
        try:
            self.policy.apply()
        except Exception:
            self.close()
            raise
        configured=dict(env)
        configured['WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS']='--remote-debugging-port='+str(self.port)
        return configured

    def attach(self, app, timeout=45):
        self.app=app;deadline=time.monotonic()+timeout
        while time.monotonic()<deadline:
            if app.poll() is not None:
                raise AssertionError('Owned application exited before live inspector readiness')
            self.diagnostics['attach_attempts']+=1
            self.diagnostics['last_stage']='owned_loopback_inspector_connect'
            candidate=None
            try:
                candidate=WindowsClient(self.port)
                label=candidate.js('window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label')
                if label!='main':
                    raise AssertionError('Measurement inspector must target only the main shell')
                self.client=candidate
                self.diagnostics['last_stage']='ready'
                return
            except (OSError,ValueError,ConnectionError,websocket.WebSocketException) as error:
                self.diagnostics['last_attach_error_type']=type(error).__name__
                if candidate: candidate.close()
                time.sleep(.15)
        raise TimeoutError('Owned read-only WebView2 observer did not become ready')

    def _call(self, command, **args):
        if command not in ('spisok','obychnye_spisok'):
            raise AssertionError('Observer must not drive application actions')
        if self.client is None or self.app is None or self.app.poll() is not None:
            raise AssertionError('Owned live session observer is unavailable')
        self.diagnostics['ipc_requests']+=1
        self.diagnostics['last_stage']='ipc_'+command
        self.client.js('window.__liveSessionReply=null;window.__TAURI_INTERNALS__.invoke('
            +json.dumps(command)+','+json.dumps(args)+').then(value=>window.__liveSessionReply={value},'
            'error=>window.__liveSessionReply={error:String(error)});true')
        deadline=time.monotonic()+5
        while time.monotonic()<deadline:
            reply=self.client.js('window.__liveSessionReply')
            if reply is not None:
                if not isinstance(reply,dict): raise AssertionError('Malformed real session IPC reply')
                if 'error' in reply:
                    message=reply['error']
                    if 'ещё запускается' in message or 'еще запускается' in message:
                        self.diagnostics['transient_unavailable']+=1
                        self.diagnostics['last_stage']='native_session_starting'
                        return None
                    raise AssertionError('Read-only real session IPC rejected: '+command+': '+message)
                self.diagnostics['last_stage']='ready'
                return reply.get('value')
            time.sleep(.02)
        raise TimeoutError('Read-only real session IPC did not finish: '+command)

    def sessions(self, owners, *, allow_dormant=False):
        records=self._call('spisok')
        if not isinstance(records,list): raise AssertionError('Real Super membership is unavailable')
        by_id={row['id']:row for row in records}
        if len(by_id)!=len(records): raise AssertionError('Duplicate native Super identity')
        result={}
        for owner in owners:
            row=by_id.get(owner)
            if row is None: raise AssertionError('Expected Super absent from actual native membership: '+owner)
            running=row.get('zapushchena')
            if type(running) is not bool: raise AssertionError('Real engine running state is missing')
            if not running and not allow_dormant:
                self.diagnostics['transient_unavailable']+=1
                self.diagnostics['last_stage']='expected_live_engine_not_ready'
                self.diagnostics['last_unavailable_owner']=owner
                return None
            session=self._call('obychnye_spisok',id=owner)
            if session is None: return None
            if not isinstance(session,dict) or not isinstance(session.get('vkladki'),list):
                raise AssertionError('Real native session snapshot is malformed')
            identifiers=[tab.get('id') for tab in session['vkladki']]
            if any(type(identifier) is not int or identifier<=0 for identifier in identifiers) or len(set(identifiers))!=len(identifiers):
                raise AssertionError('Real native session contains invalid or duplicate tab IDs')
            self.diagnostics['live_sessions' if running else 'dormant_snapshots']+=1
            result[owner]=session
        return result

    def session(self, owner, *, allow_dormant=False):
        snapshots=self.sessions([owner],allow_dormant=allow_dormant)
        return snapshots.get(owner) if snapshots is not None else None

    def close(self):
        try:
            if self.client:
                self.client.close()
                self.client=None
        finally:
            if self.policy:
                self.policy.restore()
                self.policy=None
