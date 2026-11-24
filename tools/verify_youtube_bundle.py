#!/usr/bin/env python3
"""Compare extracted helper bytes against trusted packager output; executes nothing.
Only fixed reasons, numeric counts and public content hashes enter the receipt.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import tempfile
import time

MAX_ENTRIES = 8192
MAX_BYTES = 1024 ** 3
MANIFEST_LIMIT = 128 * 1024


class Refused(Exception):
    def __init__(self, phase, reason):
        self.phase, self.reason = phase, reason
        super().__init__(reason)


def relative_name(name):
    if not isinstance(name, str) or not name or len(name) > 1024 or '\\' in name or ':' in name:
        raise Refused('tree', 'invalid_path')
    parts = name.split('/')
    reserved = {'CON', 'AUX', 'PRN', 'NUL'} | {f'{p}{i}' for p in ('COM', 'LPT') for i in range(1, 10)}
    if any(not p or p in ('.', '..') or p.endswith((' ', '.')) or '\0' in p or p.split('.')[0].upper() in reserved for p in parts):
        raise Refused('tree', 'invalid_path')
    return name


def node(path):
    info = path.lstat()
    if stat.S_ISLNK(info.st_mode) or getattr(info, 'st_file_attributes', 0) & 0x400:
        raise Refused('tree', 'symlink_or_reparse')
    return info


def deadline_check(deadline):
    if time.monotonic() >= deadline: raise Refused('bounds', 'deadline')


def inventory(root, deadline):
    if not stat.S_ISDIR(node(root).st_mode): raise Refused('tree', 'not_directory')
    files, dirs, names = {}, set(), set()
    total, count = 0, 0
    pending = [root]
    while pending:
        deadline_check(deadline)
        parent = pending.pop()
        for path in parent.iterdir():
            count += 1
            if count > MAX_ENTRIES: raise Refused('bounds', 'entry_count')
            name = relative_name(path.relative_to(root).as_posix())
            folded = name.casefold()
            if folded in names: raise Refused('tree', 'case_collision')
            names.add(folded)
            info = node(path)
            if stat.S_ISDIR(info.st_mode):
                dirs.add(name); pending.append(path)
            elif stat.S_ISREG(info.st_mode):
                total += info.st_size
                if total > MAX_BYTES: raise Refused('bounds', 'total_bytes')
                files[name] = info
            else: raise Refused('tree', 'special_file')
    return files, dirs, total


def file_digest(root, name, info, deadline):
    deadline_check(deadline)
    path = root / name
    flags = os.O_RDONLY | getattr(os, 'O_BINARY', 0) | getattr(os, 'O_NOFOLLOW', 0)
    fd = os.open(path, flags)
    h, count = hashlib.sha256(), 0
    with os.fdopen(fd, 'rb') as source:
        actual = os.fstat(source.fileno())
        if not stat.S_ISREG(actual.st_mode) or (actual.st_dev, actual.st_ino, actual.st_size) != (info.st_dev, info.st_ino, info.st_size):
            raise Refused('hash', 'file_changed')
        while True:
            deadline_check(deadline)
            block = source.read(1024 * 1024)
            if not block: break
            count += len(block)
            if count > info.st_size: raise Refused('hash', 'file_grew')
            h.update(block)
    if count != info.st_size: raise Refused('hash', 'file_changed')
    return h.hexdigest()


def manifest(expected, files):
    record = files.get('components.json')
    if not record or not 0 < record.st_size <= MANIFEST_LIMIT: raise Refused('manifest', 'missing_or_large_manifest')
    with (expected / 'components.json').open('rb') as source: data = source.read(MANIFEST_LIMIT + 1)
    if len(data) != record.st_size: raise Refused('manifest', 'manifest_changed')
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result: raise Refused('manifest', 'duplicate_key')
            result[key] = value
        return result
    try: document = json.loads(data, object_pairs_hook=unique)
    except (ValueError, UnicodeError, RecursionError): raise Refused('manifest', 'invalid_json') from None
    if (not isinstance(document, dict) or type(document.get('schema')) is not int or document['schema'] != 1
            or document.get('platform') not in ('windows-x64', 'linux-x64')
            or not isinstance(document.get('components'), dict)
            or set(document['components']) != {'yt-dlp', 'deno', 'ffmpeg'}):
        raise Refused('manifest', 'invalid_component_schema')
    names = set()
    for component in document['components'].values():
        if not isinstance(component, dict): raise Refused('manifest', 'invalid_component_schema')
        name = relative_name(component.get('filename'))
        digest = component.get('sha256')
        if name in names or name not in files or not isinstance(digest, str) or not re.fullmatch('[0-9a-f]{64}', digest):
            raise Refused('manifest', 'invalid_component_record')
        names.add(name)
    return document, hashlib.sha256(data).hexdigest()


def verify(expected, actual):
    report = dict(schema=1, accepted=False, phase='expected_tree', reason='not_completed',
                  expected_files=0, actual_files=0, expected_bytes=0, actual_bytes=0,
                  missing_files=0, extra_files=0, matched_files=0)
    deadline = time.monotonic() + 120
    try:
        left, left_dirs, report['expected_bytes'] = inventory(expected, deadline)
        report['expected_files'] = len(left)
        report['phase'] = 'manifest'
        document, report['manifest_sha256'] = manifest(expected, left)
        report['component_count'] = len(document['components'])
        report['phase'] = 'actual_tree'
        right, right_dirs, report['actual_bytes'] = inventory(actual, deadline)
        report['actual_files'] = len(right)
        report['phase'] = 'tree_closure'
        report['missing_files'], report['extra_files'] = len(left.keys() - right.keys()), len(right.keys() - left.keys())
        if left.keys() != right.keys() or left_dirs != right_dirs: raise Refused('tree_closure', 'missing_or_extra_tree_entry')
        report['phase'] = 'hash'
        digests = {}
        for name in sorted(left):
            if left[name].st_size != right[name].st_size: raise Refused('bytes', 'size_mismatch')
            digest = file_digest(expected, name, left[name], deadline)
            if digest != file_digest(actual, name, right[name], deadline): raise Refused('hash', 'hash_mismatch')
            digests[name] = digest
            report['matched_files'] += 1
        for component in document['components'].values():
            if digests[component['filename']] != component['sha256']: raise Refused('manifest', 'component_hash_mismatch')
        report.update(accepted=True, phase='complete', reason='closed_tree_identical')
    except Refused as error:
        # Preserve expected/actual stage for filesystem-type rejections.
        report.update(phase=error.phase if error.phase not in ('tree', 'bounds') else report['phase'], reason=error.reason)
    except (OSError, ValueError, TypeError, KeyError, RecursionError):
        report['reason'] = 'filesystem_or_schema_error'
    return report


def self_test():
    import shutil
    with tempfile.TemporaryDirectory(prefix='nb-bundle-test-') as temporary:
        root = Path(temporary); expected = root / 'expected'; expected.mkdir()
        components = {}
        for name, filename in [('yt-dlp', 'yt-dlp.exe'), ('deno', 'deno.exe'), ('ffmpeg', 'ffmpeg/bin/ffmpeg.exe')]:
            path = expected / filename; path.parent.mkdir(parents=True, exist_ok=True); data = name.encode(); path.write_bytes(data)
            components[name] = dict(filename=filename, sha256=hashlib.sha256(data).hexdigest())
        hidden = expected / 'licenses/transitive/repos/native/.gitmodules'
        hidden.parent.mkdir(parents=True); hidden.write_bytes(b'static hidden notice metadata')
        (expected / 'components.json').write_text(json.dumps(dict(schema=1, platform='windows-x64', components=components)))
        for case in ('valid', 'tamper', 'missing', 'extra', 'case', 'symlink'):
            actual = root / case; shutil.copytree(expected, actual)
            if case == 'tamper': (actual / 'deno.exe').write_bytes(b'xxxx')
            if case == 'missing': (actual / 'licenses/transitive/repos/native/.gitmodules').unlink()
            if case == 'extra': (actual / 'unknown').write_bytes(b'extra')
            if case == 'case': (actual / 'DENO.EXE').write_bytes(b'collision')
            if case == 'symlink':
                (actual / 'deno.exe').unlink(); (actual / 'deno.exe').symlink_to(root / 'never-opened')
            result = verify(expected, actual)
            assert result['accepted'] is (case == 'valid'), (case, result)
            assert str(root) not in json.dumps(result)
        assert verify(expected, root / 'missing-root')['accepted'] is False
    print(json.dumps(dict(self_test=True, native_execution=False)))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--expected', type=Path)
    parser.add_argument('--actual', type=Path)
    parser.add_argument('--out', type=Path)
    parser.add_argument('--self-test', action='store_true')
    args = parser.parse_args()
    if args.self_test: self_test(); return
    if not all((args.expected, args.actual, args.out)): parser.error('expected_actual_out_required')
    report = verify(args.expected, args.actual)
    try:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(json.dumps(report, indent=2) + '\n')
    except OSError: raise SystemExit('receipt_write_failed') from None
    print(json.dumps(report))
    raise SystemExit(0 if report['accepted'] else 1)

if __name__ == '__main__': main()
