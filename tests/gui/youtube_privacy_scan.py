#!/usr/bin/env python3
"""Literal UTF8/16 privacy evidence for an OWNED disposable player profile.
Call after actual owned process-tree exit. Positive marker must be cacheable
response BODY content, never a URL that could appear only in history.
No receipt includes paths, marker values, contents or raw exception messages.
This scanner does not decode compressed/encrypted Chromium storage formats.
"""
import os
from pathlib import Path
import stat
import tempfile
import time


def _patterns(value):
    if not isinstance(value,str) or not 24<=len(value)<=256 or not value.isascii() or not value.isalnum():
        raise ValueError('privacy_marker_invalid')
    return tuple(value.encode(e) for e in ('utf-8','utf-16-le','utf-16-be'))


def _cache_file(relative):
    return any(part.lower() in ('cache','cache_data','media cache','media_cache','code cache') for part in relative.parts[:-1])


def _metadata(value):
    return {'type':stat.S_IFMT(value.st_mode),'nlink':value.st_nlink,
            'volume':value.st_dev,'file_index':value.st_ino,
            'reparse':bool(getattr(value,'st_file_attributes',0)&getattr(stat,'FILE_ATTRIBUTE_REPARSE_POINT',0x400))}


def _open_readonly(path,flags):
    if os.name!='nt':return os.open(path,flags)
    import ctypes
    from ctypes import wintypes
    import msvcrt
    kernel=ctypes.WinDLL('kernel32',use_last_error=True)
    kernel.CreateFileW.argtypes=[wintypes.LPCWSTR,wintypes.DWORD,wintypes.DWORD,ctypes.c_void_p,
                                wintypes.DWORD,wintypes.DWORD,wintypes.HANDLE]
    kernel.CreateFileW.restype=wintypes.HANDLE
    kernel.CloseHandle.argtypes=[wintypes.HANDLE];kernel.CloseHandle.restype=wintypes.BOOL
    # OPEN_REPARSE_POINT prevents following a replaced node; BACKUP_SEMANTICS
    # permits observing and rejecting a directory race before reading bytes.
    handle=kernel.CreateFileW(str(path),0x80000000,7,None,3,0x00200000|0x02000000,None)
    if handle in (None,ctypes.c_void_p(-1).value):raise ctypes.WinError(ctypes.get_last_error())
    try:return msvcrt.open_osfhandle(handle,flags)
    except BaseException:
        kernel.CloseHandle(handle);raise


def _windows_info(descriptor):
    if os.name!='nt':return None
    import ctypes
    from ctypes import wintypes
    import msvcrt
    class Information(ctypes.Structure):
        _fields_=[('attributes',wintypes.DWORD),('creation',wintypes.FILETIME),
                  ('access',wintypes.FILETIME),('write',wintypes.FILETIME),
                  ('volume',wintypes.DWORD),('size_high',wintypes.DWORD),('size_low',wintypes.DWORD),
                  ('links',wintypes.DWORD),('index_high',wintypes.DWORD),('index_low',wintypes.DWORD)]
    kernel=ctypes.WinDLL('kernel32',use_last_error=True)
    kernel.GetFileInformationByHandle.argtypes=[wintypes.HANDLE,ctypes.POINTER(Information)]
    kernel.GetFileInformationByHandle.restype=wintypes.BOOL
    info=Information()
    if not kernel.GetFileInformationByHandle(wintypes.HANDLE(msvcrt.get_osfhandle(descriptor)),ctypes.byref(info)):
        raise ctypes.WinError(ctypes.get_last_error())
    return {'volume':info.volume,'file_index':(info.index_high<<32)|info.index_low,
            'number_of_links':info.links,'attributes':info.attributes,'reparse':bool(info.attributes&0x400)}



def _symlink_diagnostic(entry, root, media):
    """Describe only the link itself. Never follow or stat its target."""
    import re
    code={'SingletonLock':'singleton_lock','SingletonCookie':'cookie',
          'SingletonSocket':'socket','RunningChromeVersion':'running_chrome_version'}.get(entry.name,'other')
    # Root-level links have depth zero. No other basename is returned.
    result={'link_name_code':code,'link_depth':len(Path(entry.path).relative_to(root).parts)-1,
            'target_kind':'unknown','readlink_bounded':False}
    if os.name!='posix':return result
    try:
        import ctypes
        libc=ctypes.CDLL(None,use_errno=True)
        libc.readlink.argtypes=[ctypes.c_char_p,ctypes.c_void_p,ctypes.c_size_t]
        libc.readlink.restype=ctypes.c_ssize_t
        target=ctypes.create_string_buffer(1025)
        count=libc.readlink(os.fsencode(entry.path),target,len(target))
        if not 0<count<=1024:return result
        raw=target.raw[:count]
        result['readlink_bounded']=True
        result['contains_media_marker']=any(pattern in raw for pattern in media)
        value=raw.decode('utf-8')
        kind='other'
        if code=='singleton_lock' and re.fullmatch(r'[A-Za-z0-9_.-]{1,253}-[1-9][0-9]{0,9}',value):
            kind='host_pid'
        elif code=='cookie' and re.fullmatch(r'[A-Za-z0-9_-]{1,128}',value):
            kind='opaque_token'
        elif code=='running_chrome_version' and re.fullmatch(r'[0-9]{1,5}(?:\.[0-9]{1,5}){1,4}',value):
            kind='version'
        elif code=='socket':
            # Lexical only: no resolve(), exists(), stat() or opening the target.
            target_path=Path(value);temp=Path(tempfile.gettempdir()).absolute()
            if target_path.is_absolute() and '..' not in target_path.parts and temp in target_path.parents and target_path.name=='SingletonSocket':
                kind='system_temp_socket_path'
        result['target_kind']=kind
    except Exception:
        pass  # Original fail-closed symlink result remains authoritative.
    return result


def scan_profile(sandbox_root, profile_root, media_sentinel, positive_body_sentinel, *,
                 max_files=8192, max_bytes=512*1024*1024, max_seconds=30):
    """Scan only a subtree of a system-temp nb-player-* sandbox; never follows
    links. Caps, mutation, symlinks and IO errors make acceptance false.
    Caller must independently prove native shutdown and the cacheable control.
    """
    media,positive=_patterns(media_sentinel),_patterns(positive_body_sentinel)
    if media_sentinel in positive_body_sentinel or positive_body_sentinel in media_sentinel:
        raise ValueError('privacy_markers_overlap')
    if not(type(max_files)is int and 1<=max_files<=16384 and type(max_bytes)is int and 1<=max_bytes<=1024*1024*1024 and 0<max_seconds<=60):
        raise ValueError('privacy_scan_bounds_invalid')
    sandbox,root=Path(sandbox_root).absolute(),Path(profile_root).absolute()
    if (sandbox.parent.resolve()!=Path(tempfile.gettempdir()).resolve() or not sandbox.name.startswith('nb-player-')
            or sandbox.is_symlink() or not sandbox.is_dir() or sandbox.resolve()!=sandbox):
        raise ValueError('privacy_scan_not_disposable')
    if root!=sandbox and sandbox not in root.parents:raise ValueError('privacy_scan_scope_invalid')
    for parent in (root,*root.parents):
        if parent==sandbox:break
        if parent.is_symlink():raise ValueError('privacy_scan_scope_invalid')
    if not root.is_dir() or root.resolve()!=root:raise ValueError('privacy_scan_scope_invalid')
    receipt=dict(schema=1,complete=False,accepted=False,files=0,bytes=0,media_files=0,
                 positive_cache_files=0,positive_other_files=0,failure=None,
                 literal_encodings=['utf8','utf16le','utf16be'])
    started=time.monotonic();stack=[root];directories=0;discovered=1;overlap=max(map(len,media+positive))-1
    class ScanFailed(Exception):pass
    def fail(code):raise ScanFailed(code)
    try:
        while stack:
            if time.monotonic()-started>max_seconds:fail('deadline')
            directory=stack.pop();directories+=1
            if directories>max_files:fail('directory_bound')
            with os.scandir(directory) as entries:
                for entry in entries:
                    if time.monotonic()-started>max_seconds:fail('deadline')
                    if entry.is_symlink():
                        receipt['symlink_diagnostic']=_symlink_diagnostic(entry,root,media)
                        fail('symlink')
                    # Windows DirEntry.stat deliberately reports zero identity/link
                    # fields. Authoritative lstat and opened fstat remain strict.
                    cached=entry.stat(follow_symlinks=False)
                    before=os.stat(entry.path,follow_symlinks=False)
                    if _metadata(before)['reparse']:fail('reparse')
                    if stat.S_ISDIR(before.st_mode):
                        discovered+=1
                        if discovered>max_files:fail('directory_bound')
                        stack.append(Path(entry.path));continue
                    if not stat.S_ISREG(before.st_mode):fail('special_file')
                    node={'direntry':_metadata(cached),'stat':_metadata(before)}
                    if 'node_metadata' not in receipt or before.st_nlink!=1:receipt['node_metadata']=node
                    if before.st_nlink!=1:fail('hardlink')
                    receipt['files']+=1
                    if receipt['files']>max_files:fail('file_bound')
                    if before.st_size>max_bytes-receipt['bytes']:fail('byte_bound')
                    flags=os.O_RDONLY|getattr(os,'O_BINARY',0)|getattr(os,'O_NOFOLLOW',0)
                    descriptor=_open_readonly(entry.path,flags)
                    with os.fdopen(descriptor,'rb') as stream:
                        opened=os.fstat(stream.fileno())
                        native=_windows_info(stream.fileno())
                        node['opened']=_metadata(opened)
                        if native is not None:node['windows']=native
                        if opened.st_nlink!=1 or native is not None and native['number_of_links']!=1:
                            receipt['node_metadata']=node;fail('hardlink')
                        if _metadata(opened)['reparse'] or native is not None and native['reparse']:
                            receipt['node_metadata']=node;fail('reparse')
                        if (opened.st_dev,opened.st_ino,opened.st_size)!=(before.st_dev,before.st_ino,before.st_size):fail('file_changed')
                        tail=b'';media_hit=positive_hit=False;read=0
                        while True:
                            if time.monotonic()-started>max_seconds:fail('deadline')
                            chunk=stream.read(min(65536,max_bytes-receipt['bytes']+1))
                            if not chunk:break
                            receipt['bytes']+=len(chunk);read+=len(chunk)
                            if receipt['bytes']>max_bytes:fail('byte_bound')
                            window=tail+chunk;media_hit|=any(p in window for p in media);positive_hit|=any(p in window for p in positive);tail=window[-overlap:]
                        after=os.fstat(stream.fileno())
                        native_after=_windows_info(stream.fileno())
                        if after.st_nlink!=1 or native_after is not None and native_after['number_of_links']!=1:
                            receipt['node_metadata']=node;fail('hardlink')
                        if native_after is not None and (native_after['reparse'] or
                                (native_after['volume'],native_after['file_index'])!=(native['volume'],native['file_index'])):
                            receipt['node_metadata']=node;fail('file_changed')
                        if read!=before.st_size or (after.st_dev,after.st_ino,after.st_size,after.st_mtime_ns)!=(before.st_dev,before.st_ino,before.st_size,before.st_mtime_ns):fail('file_changed')
                    if media_hit:receipt['media_files']+=1
                    if positive_hit:receipt['positive_cache_files' if _cache_file(Path(entry.path).relative_to(root)) else 'positive_other_files']+=1
        receipt['complete']=True
        receipt['accepted']=receipt['media_files']==0 and receipt['positive_cache_files']>0
    except ScanFailed as error:receipt['failure']=str(error)
    except OSError:receipt['failure']='io_error'
    return receipt


def self_test():
    import shutil
    sandbox=Path(tempfile.mkdtemp(prefix='nb-player-')).resolve(strict=True)
    media='NexusBroMediaCanary0123456789abcd';positive='NexusBroPositiveBody9876543210abcd'
    try:
        root=sandbox/'profile';cache=root/'Default'/'Cache'/'Cache_Data';cache.mkdir(parents=True)
        (cache/'positive').write_bytes(positive.encode())
        clean=scan_profile(sandbox,root,media,positive);assert clean['complete'] and clean['accepted']
        # Real filesystem hardlink must still be refused, never accept nlink=0.
        os.link(cache/'positive',cache/'hardlink')
        assert scan_profile(sandbox,root,media,positive)['failure']=='hardlink'
        (cache/'hardlink').unlink()
        # Reproduce the Windows DirEntry cache boundary while retaining actual
        # os.stat/fstat and real profile files; no native acceptance is implied.
        from unittest.mock import patch
        from types import SimpleNamespace
        original_scandir=os.scandir
        class ZeroEntry:
            def __init__(self,entry):self.entry=entry
            def __getattr__(self,key):return getattr(self.entry,key)
            def stat(self,**kwargs):
                value=self.entry.stat(**kwargs)
                return SimpleNamespace(st_mode=value.st_mode,st_nlink=0,st_dev=0,st_ino=0,
                                       st_file_attributes=getattr(value,'st_file_attributes',0))
        class ZeroScandir:
            def __init__(self,path):self.inner=original_scandir(path)
            def __enter__(self):return (ZeroEntry(entry) for entry in self.inner)
            def __exit__(self,*args):return self.inner.__exit__(*args)
        with patch.object(os,'scandir',ZeroScandir):
            cached_zero=scan_profile(sandbox,root,media,positive)
        assert cached_zero['complete'] and cached_zero['accepted']
        assert cached_zero['node_metadata']['direntry']['nlink']==0
        assert cached_zero['node_metadata']['stat']['nlink']==cached_zero['node_metadata']['opened']['nlink']==1

        for encoding in ('utf-8','utf-16-le','utf-16-be'):
            (cache/'media').write_bytes(b'x'*65527+media.encode(encoding))
            hit=scan_profile(sandbox,root,media,positive);assert hit['complete'] and not hit['accepted'] and hit['media_files']==1
        (cache/'media').unlink()
        limited=scan_profile(sandbox,root,media,positive,max_bytes=1);assert not limited['complete'] and not limited['accepted']
        (cache/'positive').unlink();(root/'History').write_bytes(positive.encode())
        history=scan_profile(sandbox,root,media,positive);assert history['positive_other_files']==1 and not history['accepted']
        try:(cache/'link').symlink_to(root/'History')
        except OSError:pass  # Windows may not grant symlink creation to fixtures.
        else:
            import json
            refused=scan_profile(sandbox,root,media,positive)
            assert refused['failure']=='symlink' and not refused['complete'] and not refused['accepted']
            assert refused['symlink_diagnostic']['link_name_code']=='other'
            assert refused['symlink_diagnostic']['link_depth']==3
            assert str(root) not in json.dumps(refused) and 'History' not in json.dumps(refused)
            (cache/'link').unlink()
            if os.name=='posix':
                cases=[('SingletonLock','private-host-4242','singleton_lock','host_pid'),
                       ('SingletonCookie','opaqueFixture123','cookie','opaque_token'),
                       ('SingletonSocket',str(Path(tempfile.gettempdir())/'private-fixture'/'SingletonSocket'),'socket','system_temp_socket_path'),
                       ('RunningChromeVersion','156.0.8078.88','running_chrome_version','version')]
                for name,target,name_code,target_kind in cases:
                    link=root/name;link.symlink_to(target)
                    receipt=scan_profile(sandbox,root,media,positive)
                    detail=receipt['symlink_diagnostic']
                    assert receipt['failure']=='symlink' and not receipt['complete'] and not receipt['accepted']
                    assert detail=={'link_name_code':name_code,'link_depth':0,'target_kind':target_kind,
                                    'readlink_bounded':True,'contains_media_marker':False}
                    assert target not in json.dumps(receipt) and str(root) not in json.dumps(receipt)
                    link.unlink()
                (root/'SingletonCookie').symlink_to(media)
                marked=scan_profile(sandbox,root,media,positive)
                assert marked['symlink_diagnostic']['contains_media_marker'] is True
                assert not marked['complete'] and not marked['accepted'] and media not in json.dumps(marked)
                (root/'SingletonCookie').unlink()
                (root/'SingletonCookie').symlink_to('x'*1100)
                oversized=scan_profile(sandbox,root,media,positive)
                assert oversized['symlink_diagnostic']['target_kind']=='unknown'
                assert oversized['symlink_diagnostic']['readlink_bounded'] is False
                assert not oversized['complete'] and not oversized['accepted']
                (root/'SingletonCookie').unlink()
        try:scan_profile(Path.home(),root,media,positive)
        except ValueError as error:assert str(error)=='privacy_scan_not_disposable'
        else:raise AssertionError('owner root accepted')
        assert media not in str(clean) and str(root) not in str(clean)
    finally:shutil.rmtree(sandbox)
    return dict(self_test=True,synthetic_only=True,native_execution=False)


if __name__=='__main__':
    import json
    print(json.dumps(self_test()))
