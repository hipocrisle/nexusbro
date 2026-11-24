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
                    if entry.is_symlink():fail('symlink')
                    before=entry.stat(follow_symlinks=False)
                    if stat.S_ISDIR(before.st_mode):
                        discovered+=1
                        if discovered>max_files:fail('directory_bound')
                        stack.append(Path(entry.path));continue
                    if not stat.S_ISREG(before.st_mode):fail('special_file')
                    if before.st_nlink!=1:fail('hardlink')
                    receipt['files']+=1
                    if receipt['files']>max_files:fail('file_bound')
                    if before.st_size>max_bytes-receipt['bytes']:fail('byte_bound')
                    flags=os.O_RDONLY|getattr(os,'O_BINARY',0)|getattr(os,'O_NOFOLLOW',0)
                    descriptor=os.open(entry.path,flags)
                    with os.fdopen(descriptor,'rb') as stream:
                        opened=os.fstat(stream.fileno())
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
                        if read!=before.st_size or (after.st_size,after.st_mtime_ns)!=(before.st_size,before.st_mtime_ns):fail('file_changed')
                    if media_hit:receipt['media_files']+=1
                    if positive_hit:receipt['positive_cache_files' if _cache_file(Path(entry.path).relative_to(root)) else 'positive_other_files']+=1
        receipt['complete']=True
        receipt['accepted']=receipt['media_files']==0 and receipt['positive_cache_files']>0
    except ScanFailed as error:receipt['failure']=str(error)
    except OSError:receipt['failure']='io_error'
    return receipt


def self_test():
    import shutil
    sandbox=Path(tempfile.mkdtemp(prefix='nb-player-'))
    media='NexusBroMediaCanary0123456789abcd';positive='NexusBroPositiveBody9876543210abcd'
    try:
        root=sandbox/'profile';cache=root/'Default'/'Cache'/'Cache_Data';cache.mkdir(parents=True)
        (cache/'positive').write_bytes(positive.encode())
        clean=scan_profile(sandbox,root,media,positive);assert clean['complete'] and clean['accepted']
        for encoding in ('utf-8','utf-16-le','utf-16-be'):
            (cache/'media').write_bytes(b'x'*65527+media.encode(encoding))
            hit=scan_profile(sandbox,root,media,positive);assert hit['complete'] and not hit['accepted'] and hit['media_files']==1
        (cache/'media').unlink()
        limited=scan_profile(sandbox,root,media,positive,max_bytes=1);assert not limited['complete'] and not limited['accepted']
        (cache/'positive').unlink();(root/'History').write_bytes(positive.encode())
        history=scan_profile(sandbox,root,media,positive);assert history['positive_other_files']==1 and not history['accepted']
        try:(cache/'link').symlink_to(root/'History')
        except OSError:pass  # Windows may not grant symlink creation to fixtures.
        else:assert scan_profile(sandbox,root,media,positive)['failure']=='symlink'
        try:scan_profile(Path.home(),root,media,positive)
        except ValueError as error:assert str(error)=='privacy_scan_not_disposable'
        else:raise AssertionError('owner root accepted')
        assert media not in str(clean) and str(root) not in str(clean)
    finally:shutil.rmtree(sandbox)
    return dict(self_test=True,synthetic_only=True,native_execution=False)


if __name__=='__main__':
    import json
    print(json.dumps(self_test()))
