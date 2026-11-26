#!/usr/bin/env python3
"""Pure byte-level verifier for the controlled media fixture artifact.

Runs AFTER actions/download-artifact and BEFORE repackaging. Checks only
bytes/structure of the four expected files — no FFmpeg/ffprobe, no codec or
playback claims, no network. Failure prints a fixed enum code only (no file
content, no raw paths) and exits 1.

Contract:
- directory contains EXACTLY video.webm, audio.webm, video-high.webm,
  SHA256SUMS — regular files, link count 1, no symlinks (checked by lstat
  before open and by fstat identity after reading);
- each .webm is 1..16 MiB and starts with the EBML magic 1a 45 df a3;
- SHA256SUMS <= 16 KiB, exactly three lines "64 lowercase hex + two spaces +
  .dev-check/youtube-media-fixture/<fixed name>", no duplicates, no foreign
  or traversal paths, all three fixed names present;
- streamed digests must match the manifest.
"""
import argparse
import hashlib
import json
import os
import stat
import sys

FIXTURE_PREFIX = '.dev-check/youtube-media-fixture/'
NAMES = ('video.webm', 'audio.webm', 'video-high.webm')
MANIFEST = 'SHA256SUMS'
MIN_WEBM = 1
MAX_WEBM = 16 * 1024 * 1024
MAX_MANIFEST = 16 * 1024
EBML_MAGIC = b'\x1a\x45\xdf\xa3'
HEX = set('0123456789abcdef')


class Rejected(Exception):
    def __init__(self, code):
        super().__init__(code)
        self.code = code


def _lstat_regular(path):
    try:
        info = os.lstat(path)
    except OSError:
        raise Rejected('entry_missing')
    if stat.S_ISLNK(info.st_mode):
        raise Rejected('symlink_entry')
    if not stat.S_ISREG(info.st_mode):
        raise Rejected('non_regular')
    if info.st_nlink != 1:
        raise Rejected('link_count')
    return info


def _read_checked(path, cap):
    """Stream the file with a hard cap; verify identity (dev, ino) after."""
    before = _lstat_regular(path)
    digest = hashlib.sha256()
    total = 0
    head = b''
    try:
        with open(path, 'rb') as stream:
            opened = os.fstat(stream.fileno())
            if opened.st_nlink != 1 or not stat.S_ISREG(opened.st_mode) or (opened.st_dev, opened.st_ino) != (before.st_dev, before.st_ino):
                raise Rejected('changed_during_read')
            while True:
                chunk = stream.read(1024 * 1024)
                if not chunk:
                    break
                total += len(chunk)
                if total > cap:
                    raise Rejected('size_out_of_range')
                if len(head) < 4:
                    head += chunk[:4 - len(head)]
                digest.update(chunk)
            after = os.fstat(stream.fileno())
            if after.st_nlink != 1 or (after.st_dev, after.st_ino, after.st_size) != (opened.st_dev, opened.st_ino, opened.st_size):
                raise Rejected('changed_during_read')
    except OSError:
        raise Rejected('read_error')
    return digest.hexdigest(), total, head


def _parse_manifest(data):
    if len(data) > MAX_MANIFEST:
        raise Rejected('manifest_size')
    try:
        text = data.decode('ascii')
    except UnicodeDecodeError:
        raise Rejected('manifest_format')
    lines = text.splitlines()
    if len(lines) != 3:
        raise Rejected('manifest_format')
    entries = {}
    for line in lines:
        digest, sep, rest = line[:64], line[64:66], line[66:]
        if len(digest) != 64 or any(c not in HEX for c in digest) or sep != '  ':
            raise Rejected('manifest_format')
        if not rest.startswith(FIXTURE_PREFIX):
            raise Rejected('manifest_path')
        name = rest[len(FIXTURE_PREFIX):]
        if name not in NAMES or '/' in name or '\\' in name or '..' in name:
            raise Rejected('manifest_path')
        if name in entries:
            raise Rejected('manifest_duplicate')
        entries[name] = digest
    if set(entries) != set(NAMES):
        raise Rejected('manifest_missing')
    return entries


def verify(directory):
    if os.path.islink(directory) or not os.path.isdir(directory):
        raise Rejected('directory_invalid')
    try:
        listing = set(os.listdir(directory))
    except OSError:
        raise Rejected('directory_invalid')
    expected = set(NAMES) | {MANIFEST}
    if listing != expected:
        raise Rejected('entry_set_mismatch')
    manifest_info = _lstat_regular(os.path.join(directory, MANIFEST))
    if manifest_info.st_size > MAX_MANIFEST:
        raise Rejected('manifest_size')
    try:
        with open(os.path.join(directory, MANIFEST), 'rb') as stream:
            opened = os.fstat(stream.fileno())
            if opened.st_nlink != 1 or (opened.st_dev, opened.st_ino, opened.st_size) != (manifest_info.st_dev, manifest_info.st_ino, manifest_info.st_size):
                raise Rejected('changed_during_read')
            manifest_raw = stream.read(MAX_MANIFEST + 1)
            after = os.fstat(stream.fileno())
            if after.st_nlink != 1 or (after.st_dev, after.st_ino, after.st_size) != (opened.st_dev, opened.st_ino, opened.st_size):
                raise Rejected('changed_during_read')
    except OSError:
        raise Rejected('read_error')
    if len(manifest_raw) > MAX_MANIFEST:
        raise Rejected('manifest_size')
    entries = _parse_manifest(manifest_raw)
    report = {'verified': True, 'files': {}}
    for name in NAMES:
        path = os.path.join(directory, name)
        info = _lstat_regular(path)
        if not MIN_WEBM <= info.st_size <= MAX_WEBM:
            raise Rejected('size_out_of_range')
        digest, total, head = _read_checked(path, MAX_WEBM)
        if total != info.st_size:
            raise Rejected('changed_during_read')
        if head != EBML_MAGIC:
            raise Rejected('ebml_magic')
        if digest != entries[name]:
            raise Rejected('digest_mismatch')
        report['files'][name] = {'sha256': digest, 'bytes': total}
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory', required=True)
    parser.add_argument('--out')
    args = parser.parse_args()
    try:
        report = verify(args.directory)
    except Rejected as error:
        print(json.dumps({'verified': False, 'code': error.code}))
        return 1
    payload = json.dumps(report, indent=2, sort_keys=True) + '\n'
    if args.out:
        with open(args.out, 'w', encoding='utf-8') as stream:
            stream.write(payload)
    print(json.dumps({'verified': True, 'files': len(report['files'])}))
    return 0


if __name__ == '__main__':
    sys.exit(main())
