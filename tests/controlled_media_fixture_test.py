#!/usr/bin/env python3
"""Offline tests for verify_controlled_media_fixture (tmp sparse files only)."""
import hashlib
import json
import os
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'tests'))
import verify_controlled_media_fixture as v

WEBM_SIZE = 1024 * 1024  # минимально допустимый размер (sparse — без аллокации)


def sparse_webm(path, magic=v.EBML_MAGIC, size=WEBM_SIZE):
    with open(path, 'wb') as stream:
        stream.write(magic)
        stream.seek(size - 1)
        stream.write(b'\0')


def sha(path):
    digest = hashlib.sha256()
    with open(path, 'rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


def manifest_lines(directory, override=None):
    lines = []
    for name in v.NAMES:
        digest = (override or {}).get(name) or sha(os.path.join(directory, name))
        lines.append(f"{digest}  {v.FIXTURE_PREFIX}{name}")
    return '\n'.join(lines) + '\n'


def make_fixture(root, override=None):
    directory = Path(root) / 'fixture'
    directory.mkdir()
    for name in v.NAMES:
        sparse_webm(directory / name)
    (directory / v.MANIFEST).write_text(manifest_lines(directory, override))
    return directory


def expect(directory, code):
    try:
        v.verify(str(directory))
    except v.Rejected as error:
        return error.code == code
    return False


def main():
    checks = {}
    with tempfile.TemporaryDirectory() as root:
        good = make_fixture(root)
        report = v.verify(str(good))
        checks['valid_passes'] = report['verified'] is True and set(report['files']) == set(v.NAMES)
        checks['report_shape'] = all(set(f) == {'sha256', 'bytes'} and f['bytes'] == WEBM_SIZE
                                     for f in report['files'].values())
        checks['report_no_paths'] = '/tmp' not in json.dumps(report) and str(root) not in json.dumps(report)

    with tempfile.TemporaryDirectory() as root:
        d = make_fixture(root)
        (d / 'video.webm').write_bytes(b'')
        sparse_webm(d / 'video.webm')
        (d / v.MANIFEST).write_text(manifest_lines(d, {'video.webm': '0' * 64}))
        checks['tampered_sha'] = expect(d, 'digest_mismatch')

    with tempfile.TemporaryDirectory() as root:
        d = make_fixture(root)
        sparse_webm(d / 'video.webm', magic=b'\x00\x00\x00\x00')
        (d / v.MANIFEST).write_text(manifest_lines(d))
        checks['wrong_header'] = expect(d, 'ebml_magic')

    with tempfile.TemporaryDirectory() as root:
        d = make_fixture(root)
        os.truncate(d / 'video.webm', v.MAX_WEBM + 1)
        checks['too_large'] = expect(d, 'size_out_of_range')

    with tempfile.TemporaryDirectory() as root:
        d = make_fixture(root)
        os.truncate(d / 'video.webm', v.MIN_WEBM - 1)
        checks['too_small'] = expect(d, 'size_out_of_range')

    with tempfile.TemporaryDirectory() as root:
        d = make_fixture(root)
        os.unlink(d / 'audio.webm')
        checks['missing_file'] = expect(d, 'entry_set_mismatch')

    with tempfile.TemporaryDirectory() as root:
        d = make_fixture(root)
        (d / 'extra.bin').write_bytes(b'x')
        checks['extra_file'] = expect(d, 'entry_set_mismatch')

    with tempfile.TemporaryDirectory() as root:
        d = make_fixture(root)
        os.unlink(d / 'video.webm')
        os.symlink(d / 'audio.webm', d / 'video.webm')
        checks['symlink_file'] = expect(d, 'symlink_entry')

    with tempfile.TemporaryDirectory() as root:
        target = make_fixture(root)
        link = Path(root) / 'linkdir'
        os.symlink(target, link)
        checks['symlink_dir'] = expect(link, 'directory_invalid')

    with tempfile.TemporaryDirectory() as root:
        d = make_fixture(root)
        os.unlink(d / 'video.webm')
        os.mkdir(d / 'video.webm')
        checks['directory_entry'] = expect(d, 'non_regular')

    with tempfile.TemporaryDirectory() as root:
        d = make_fixture(root)
        os.link(d / 'video.webm', Path(root) / 'hardlink')
        checks['hardlink_count'] = expect(d, 'link_count')

    for bad_line, code in [
        ('not-hex', 'manifest_format'),
        ('a' * 64 + ' ' + v.FIXTURE_PREFIX + 'video.webm', 'manifest_format'),  # один пробел
        ('A' * 64 + '  ' + v.FIXTURE_PREFIX + 'video.webm', 'manifest_format'),  # верхний регистр
        ('a' * 64 + '  ' + 'other/paths/video.webm', 'manifest_path'),
        ('a' * 64 + '  ' + v.FIXTURE_PREFIX + '../escape.webm', 'manifest_path'),
        ('a' * 64 + '  ' + v.FIXTURE_PREFIX + 'foreign.webm', 'manifest_path'),
    ]:
        with tempfile.TemporaryDirectory() as root:
            d = make_fixture(root)
            lines = manifest_lines(d).splitlines()
            lines[0] = bad_line
            (d / v.MANIFEST).write_text('\n'.join(lines) + '\n')
            checks['manifest_' + code + '_' + str(abs(hash(bad_line)) % 997)] = expect(d, code)

    with tempfile.TemporaryDirectory() as root:
        d = make_fixture(root)
        lines = manifest_lines(d).splitlines()
        (d / v.MANIFEST).write_text('\n'.join(lines[:2] + [lines[0]]) + '\n')
        checks['manifest_duplicate'] = expect(d, 'manifest_duplicate')

    with tempfile.TemporaryDirectory() as root:
        d = make_fixture(root)
        lines = manifest_lines(d).splitlines()
        (d / v.MANIFEST).write_text('\n'.join(lines[:2]) + '\n')
        checks['manifest_missing_entry'] = expect(d, 'manifest_format')

    with tempfile.TemporaryDirectory() as root:
        d = make_fixture(root)
        (d / v.MANIFEST).write_bytes(b'x' * (v.MAX_MANIFEST + 1))
        checks['manifest_too_big'] = expect(d, 'manifest_size')

    with tempfile.TemporaryDirectory() as root:
        checks['directory_missing'] = expect(Path(root) / 'nope', 'directory_invalid')

    # CLI: код только из фиксированного enum, без сырых путей
    import subprocess
    with tempfile.TemporaryDirectory() as root:
        d = make_fixture(root)
        os.unlink(d / 'video.webm')
        proc = subprocess.run([sys.executable,
                               str(Path(__file__).resolve().parents[1] / 'tests' / 'verify_controlled_media_fixture.py'),
                               '--directory', str(d)], capture_output=True, text=True, timeout=60)
        out = json.loads(proc.stdout)
        checks['cli_fail_code_only'] = proc.returncode == 1 and out == {'verified': False, 'code': 'entry_set_mismatch'}
        checks['cli_no_path_leak'] = str(d) not in proc.stdout and str(d) not in proc.stderr
    with tempfile.TemporaryDirectory() as root:
        d = make_fixture(root)
        out_file = Path(root) / 'receipt.json'
        proc = subprocess.run([sys.executable,
                               str(Path(__file__).resolve().parents[1] / 'tests' / 'verify_controlled_media_fixture.py'),
                               '--directory', str(d), '--out', str(out_file)],
                              capture_output=True, text=True, timeout=60)
        saved = json.loads(out_file.read_text())
        checks['cli_success_receipt'] = proc.returncode == 0 and saved['verified'] is True \
            and set(saved['files']) == set(v.NAMES)

    failed = [name for name, ok in checks.items() if not ok]
    print(json.dumps({'media_verify_checks': 'PASS' if not failed else 'FAIL',
                      'total': len(checks), 'failed': failed}))
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main())
