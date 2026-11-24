#!/usr/bin/env python3
"""CI-only source bootstrap and verification for the minimal native muxer.
No product pin is changed; source SHA is a review receipt until explicitly pinned.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import shlex
import subprocess
import tarfile
import urllib.request

COMMIT = 'a8029b6528fecbcb5d99b331cfc76e6467b9bc77'
URL = f'https://codeload.github.com/FFmpeg/FFmpeg/tar.gz/{COMMIT}'
ROOT = Path('.dev-check/muxer-build')
CONFIGURE = [
    '--disable-autodetect', '--disable-everything', '--disable-network',
    '--disable-doc', '--disable-debug', '--disable-x86asm',
    '--enable-shared', '--disable-static', '--enable-ffmpeg', '--enable-ffprobe',
    '--disable-ffplay', '--enable-protocol=file,pipe',
    '--enable-demuxer=matroska', '--enable-muxer=webm', '--enable-parser=vp9,opus',
    # WebM's VP9 stream-copy path auto-inserts this filter even without decoding.
    '--enable-bsf=vp9_superframe',
]

def digest(path):
    h = hashlib.sha256()
    with path.open('rb') as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b''):
            h.update(chunk)
    return h.hexdigest()


def safe_member(name):
    p = PurePosixPath(name)
    if p.is_absolute() or '\\' in name or any(x in ('..', '') for x in p.parts):
        raise ValueError('unsafe_source_member')
    if not p.parts or p.parts[0] != f'FFmpeg-{COMMIT}':
        raise ValueError('unexpected_source_root')
    return p


def fetch(expected):
    ROOT.mkdir(parents=True, exist_ok=True)
    archive = ROOT / 'source.tar.gz'
    part = ROOT / 'source.download'
    count = 0
    try:
        with urllib.request.urlopen(URL, timeout=30) as response, part.open('xb') as out:
            while chunk := response.read(1024 * 1024):
                count += len(chunk)
                if count > 64 * 1024 * 1024:
                    raise ValueError('source_archive_bound')
                out.write(chunk)
        actual = digest(part)
        if expected and actual != expected:
            raise ValueError('source_sha256_mismatch')
        part.replace(archive)
    finally:
        part.unlink(missing_ok=True)
    # Hash computed BEFORE archive parsing. Empty expected is explicitly only
    # bootstrap, never a trusted product source pin.
    receipt = dict(source_commit=COMMIT, source_url=URL, source_sha256=actual,
                   source_pin_verified=bool(expected), bootstrap_only=not bool(expected),
                   configure=CONFIGURE, build_metadata_files=[dict(filename='VERSION', text='git-'+COMMIT[:12]+'\n')])
    (ROOT / 'source-receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
    source = ROOT / 'source'
    source.mkdir()
    with tarfile.open(archive, 'r:gz') as tar:
        members = tar.getmembers()
        if len(members) > 20000 or sum(m.size for m in members) > 256 * 1024 * 1024:
            raise ValueError('source_expanded_bound')
        for member in members:
            rel = safe_member(member.name)
            if not member.isdir() and not member.isfile():
                raise ValueError('source_special_entry')
            target = source.joinpath(*rel.parts)
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
            else:
                target.parent.mkdir(parents=True, exist_ok=True)
                with tar.extractfile(member) as incoming, target.open('xb') as out:
                    shutil.copyfileobj(incoming, out, 1024 * 1024)
                target.chmod(0o755 if member.mode & 0o111 else 0o644)
    # Exact upstream ffbuild/version.sh reads VERSION in preference to its git
    # discovery. Prevent it from picking the enclosing NexusBro checkout HEAD.
    # This recipe-only metadata override does not alter the verified tarball.
    version_file = source / ('FFmpeg-' + COMMIT) / 'VERSION'
    version_file.write_text('git-' + COMMIT[:12] + '\n', encoding='ascii')



def failure_receipt(stage, stdout=b'', stderr=b'', *, returncode=None, timeout=False, os_error=False, errno=None):
    if not re.fullmatch('[a-z0-9_-]{1,48}', stage): raise ValueError('invalid_verification_stage')
    ROOT.mkdir(parents=True, exist_ok=True)
    # Commands have only fixed CI fixtures/local build inputs. Never store argv.
    stdout, stderr = stdout or b'', stderr or b''
    limit = 32 * 1024
    (ROOT / ('failure-' + stage + '-stdout.txt')).write_bytes(stdout[:limit])
    (ROOT / ('failure-' + stage + '-stderr.txt')).write_bytes(stderr[:limit])
    (ROOT / ('failure-' + stage + '.json')).write_text(json.dumps(dict(
        stage=stage, returncode=returncode, timeout=timeout, os_error=os_error, errno=errno,
        stdout_bytes=len(stdout), stderr_bytes=len(stderr),
        truncated=len(stdout)>limit or len(stderr)>limit)) + '\n')


def run(args, env, timeout=30, *, stage):
    try:
        result = subprocess.run([str(a) for a in args], env=env, stdin=subprocess.DEVNULL,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=timeout)
    except subprocess.TimeoutExpired as error:
        failure_receipt(stage, error.stdout, error.stderr, timeout=True)
        raise ValueError('native_verification_timeout') from None
    except OSError as error:
        failure_receipt(stage, os_error=True, errno=error.errno)
        raise ValueError('native_verification_spawn_failed') from None
    if len(result.stdout) + len(result.stderr) > 4 * 1024 * 1024 or result.returncode:
        failure_receipt(stage, result.stdout, result.stderr, returncode=result.returncode)
        raise ValueError('native_verification_failed')
    return result.stdout + result.stderr


def verify_license(data):
    # FFmpeg wraps its fixed LGPL prose after "Public"; whitespace is layout,
    # but the actual license family AND version must both remain explicit.
    text = b' '.join(data.split())
    if (b'GNU Lesser General Public License' not in text
            or b'version 2.1 of the License' not in text):
        raise ValueError('missing_lgpl_license')


def self_test():
    verify_license(b'GNU Lesser General Public\nLicense either\nversion 2.1 of the License')
    for refused in (b'GNU General Public License version 2.1 of the License',
                    b'GNU Lesser General Public License version 3 of the License', b''):
        try: verify_license(refused)
        except ValueError: pass
        else: raise AssertionError('wrong_license_accepted')
    assert '--enable-protocol=file,pipe' in shlex.split("--enable-protocol='file,pipe'")
    assert '--enable-bsf=vp9_superframe' in CONFIGURE
    # Exercise the actual runner without executing a native binary on DE-1.
    import tempfile
    from unittest.mock import patch
    with tempfile.TemporaryDirectory() as directory, patch.dict(globals(), ROOT=Path(directory)):
        failure = subprocess.CompletedProcess([], 7, b'o' * 40000, b'e' * 40000)
        with patch.object(subprocess, 'run', return_value=failure):
            try: run(['fixed-fixture'], {}, stage='stream-copy')
            except ValueError as error: assert str(error) == 'native_verification_failed'
            else: raise AssertionError('failed_native_accepted')
        receipt = json.loads((ROOT / 'failure-stream-copy.json').read_text())
        assert receipt['returncode'] == 7 and receipt['truncated'] and not receipt['timeout']
        assert sum((ROOT / ('failure-stream-copy-' + name + '.txt')).stat().st_size for name in ('stdout', 'stderr')) == 65536
        expired = subprocess.TimeoutExpired(['fixed-fixture'], 1, output=b'partial-out', stderr=b'partial-error')
        with patch.object(subprocess, 'run', side_effect=expired):
            try: run(['fixed-fixture'], {}, stage='ffprobe-streams')
            except ValueError as error: assert str(error) == 'native_verification_timeout'
            else: raise AssertionError('native_timeout_accepted')
        assert json.loads((ROOT / 'failure-ffprobe-streams.json').read_text())['timeout']
        assert 'argv' not in receipt
        with patch.object(subprocess, 'run', side_effect=PermissionError(13, 'private-path-not-published')):
            try: run(['fixed-fixture'], {}, stage='ffmpeg-version')
            except ValueError as error: assert str(error) == 'native_verification_spawn_failed'
            else: raise AssertionError('spawn_failure_accepted')
        denied = (ROOT / 'failure-ffmpeg-version.json').read_text()
        assert json.loads(denied)['errno'] == 13 and 'private-path' not in denied




def package(platform, media):
    install = (ROOT / 'install').resolve()
    bundle = ROOT / 'package' / 'ffmpeg'
    bundle.mkdir(parents=True)
    for sub in ('bin', 'lib', 'licenses'):
        (bundle / sub).mkdir()
    windows = platform == 'windows-x64'
    exe = '.exe' if windows else ''
    for name in ('ffmpeg', 'ffprobe'):
        shutil.copy2(install / 'bin' / (name + exe), bundle / 'bin' / (name + exe))
    if windows:
        libs = sorted((install / 'bin').glob('*.dll'))
        if not libs or any(not re.fullmatch(r'(avcodec|avformat|avutil|avfilter|avdevice|swresample|swscale)-\d+\.dll', p.name) for p in libs):
            raise ValueError('unexpected_dll_closure')
        for path in libs:
            if path.is_symlink(): raise ValueError('unexpected_dll_link')
            shutil.copy2(path, bundle / 'bin' / path.name)
    else:
        # Materialize exactly the ABI SONAME, not linker/full-version aliases.
        libs = sorted((install / 'lib').glob('lib*.so.*'))
        selected = [p for p in libs if re.fullmatch(r'lib(?:avcodec|avformat|avutil|avfilter|avdevice|swresample|swscale)\.so\.\d+', p.name)]
        if not selected: raise ValueError('missing_soname_closure')
        for path in selected:
            actual = path.resolve(strict=True)
            if actual.parent != install / 'lib' or not actual.is_file():
                raise ValueError('escaping_soname')
            shutil.copy2(actual, bundle / 'lib' / path.name)
    source = ROOT / 'source' / f'FFmpeg-{COMMIT}'
    for name in ('COPYING.LGPLv2.1', 'COPYING.LGPLv3', 'COPYING.GPLv2', 'COPYING.GPLv3', 'LICENSE.md'):
        shutil.copy2(source / name, bundle / 'licenses' / name)
    verify_bundle(platform, media, bundle)


def verify_bundle(platform, media, bundle, *, existing=False):
    windows = platform == 'windows-x64'
    exe = '.exe' if windows else ''
    bundle = bundle.resolve(strict=True)
    # A receipt is emitted BEFORE execution. This is GHA artifact provenance
    # evidence, not approval of a product component pin.
    inventory = []
    total = 0
    for path in sorted(bundle.rglob('*')):
        if path.is_symlink(): raise ValueError('existing_package_symlink')
        if path.is_file():
            total += path.stat().st_size
            if len(inventory) >= 256 or total > 512*1024*1024: raise ValueError('package_bound')
            inventory.append(dict(filename=path.relative_to(bundle).as_posix(), bytes=path.stat().st_size, sha256=digest(path)))
    if not inventory or len(inventory)>256 or sum(x['bytes'] for x in inventory)>512*1024*1024:
        raise ValueError('package_bound')
    ROOT.mkdir(parents=True, exist_ok=True)
    (ROOT / 'pre-execution-inventory.json').write_text(json.dumps(dict(files=inventory, product_trust_approved=False), indent=2)+'\n')
    env = {k: os.environ[k] for k in ('SystemRoot', 'WINDIR', 'TEMP', 'TMP', 'TMPDIR') if k in os.environ}
    env['PATH' if windows else 'LD_LIBRARY_PATH'] = str((bundle / ('bin' if windows else 'lib')).resolve())
    ffmpeg = (bundle / 'bin' / ('ffmpeg' + exe)).resolve()
    ffprobe = (bundle / 'bin' / ('ffprobe' + exe)).resolve()
    if existing and not windows:
        # GitHub artifact ZIP transport does not preserve Unix executable mode.
        # Only these two trusted inventoried fixed binaries may regain u+x.
        for executable in (ffmpeg, ffprobe):
            if not executable.is_file() or executable.parent != bundle / 'bin':
                raise ValueError('existing_executable_invalid')
            executable.chmod(executable.stat().st_mode | 0o100)

    receipts = {}
    for option in ('-version', '-buildconf', '-L', '-protocols', '-demuxers', '-muxers', '-decoders', '-encoders'):
        data = run([ffmpeg, '-hide_banner', option], env, stage='ffmpeg-'+option.lstrip('-'))
        (ROOT / ('verified' + option + '.txt')).write_bytes(data)
        receipts[option] = hashlib.sha256(data).hexdigest()
        if option == '-buildconf':
            text = data.decode('utf-8', 'replace')
            if any(x in text for x in ('--enable-gpl', '--enable-nonfree', '--enable-version3')):
                raise ValueError('forbidden_build_configuration')
            options = shlex.split(text)
            for required in CONFIGURE:
                if required not in options: raise ValueError('missing_build_configuration')
        if option == '-L':
            verify_license(data)
    # Same-run controlled media hashes, never user/YouTube media.
    hashes = {}
    for line in (media / 'SHA256SUMS').read_text().splitlines():
        sha, filename = line.split(maxsplit=1)
        base = PurePosixPath(filename.lstrip('*')).name
        if base in ('video.webm', 'audio.webm') and re.fullmatch('[0-9a-f]{64}', sha):
            hashes[base] = sha
    for name in ('video.webm', 'audio.webm'):
        if hashes.get(name) != digest(media / name): raise ValueError('fixture_hash_mismatch')
    output = ROOT / 'smoke.webm'
    args = [ffmpeg, '-hide_banner', '-loglevel', 'error', '-nostdin', '-n']
    for name in ('video.webm', 'audio.webm'):
        args += ['-protocol_whitelist', 'file,pipe', '-f', 'matroska', '-i', (media / name).resolve()]
    args += ['-map', '0:v:0', '-map', '1:a:0', '-c', 'copy', '-f', 'webm', output.resolve()]
    run(args, env, 90, stage="stream-copy")
    data = run([ffprobe, '-v', 'error', '-protocol_whitelist', 'file,pipe', '-f', 'matroska', '-show_entries', 'stream=codec_name,codec_type', '-of', 'json', output.resolve()], env, stage='ffprobe-streams')
    try: streams = json.loads(data)['streams']
    except (ValueError, KeyError, TypeError):
        failure_receipt('ffprobe-streams', data, returncode=0)
        raise ValueError('smoke_streams_invalid') from None
    if sorted((s['codec_type'], s['codec_name']) for s in streams) != [('audio', 'opus'), ('video', 'vp9')]:
        failure_receipt('ffprobe-streams', data, returncode=0)
        raise ValueError('smoke_codec_mismatch')
    # Protocol absent at compile time: refusal must be protocol-related, not a
    # successful connection or TLS bypass. Fixed unreachable address, no URL log.
    try:
        denied = subprocess.run([str(ffmpeg), '-hide_banner', '-nostdin', '-i', 'http://127.0.0.1:9/refused', '-f', 'webm', '-'], env=env,
                                stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=5)
    except subprocess.TimeoutExpired as error:
        failure_receipt('http-refusal', error.stdout, error.stderr, timeout=True)
        raise ValueError('http_refusal_timeout') from None
    except OSError as error:
        failure_receipt('http-refusal', os_error=True, errno=error.errno)
        raise ValueError('http_refusal_spawn_failed') from None
    if denied.returncode == 0 or b'Protocol not found' not in denied.stderr:
        failure_receipt('http-refusal', denied.stdout, denied.stderr, returncode=denied.returncode)
        raise ValueError('http_not_compile_time_refused')
    # External dependencies remain visible; Windows report generated separately
    # by trusted MSVC dumpbin, Linux loader report uses controlled built files.
    if not windows:
        report = run(['/usr/bin/ldd', ffmpeg], env, 10, stage='runtime-dependencies')
        if b'not found' in report:
            failure_receipt('runtime-dependencies', report)
            raise ValueError('unresolved_runtime_library')
        (ROOT / 'runtime-dependencies.txt').write_bytes(report)
    if existing:
        (ROOT / 'smoke-summary.json').write_text(json.dumps(dict(native_smoke=True, streams=2, vp9=True, opus=True, http_refused=True, existing_artifact=True, product_trust_approved=False)) + '\n')
        return
    files = []
    for path in sorted(bundle.rglob('*')):
        if path.is_file():
            files.append(dict(filename='ffmpeg/' + path.relative_to(bundle).as_posix(), sha256=digest(path), bytes=path.stat().st_size))
    if len(files) > 256 or sum(x['bytes'] for x in files) > 512 * 1024 * 1024:
        raise ValueError('package_bound')
    receipt = json.loads((ROOT / 'source-receipt.json').read_text())
    receipt.update(platform=platform, files=files, filename='ffmpeg/bin/ffmpeg' + exe,
                   ffprobe_filename='ffmpeg/bin/ffprobe' + exe, sha256=digest(ffmpeg),
                   native_smoke=True, network_protocol_refused=True, verification=receipts,
                   license='LGPL-2.1-or-later', runtime_dependency_inventory='runtime-dependencies.txt',
                   product_trust_approved=False)
    (ROOT / 'package' / 'components.json').write_text(json.dumps({'ffmpeg': receipt}, indent=2) + '\n')
    (ROOT / 'smoke-summary.json').write_text(json.dumps(dict(native_smoke=True, streams=2, vp9=True, opus=True, http_refused=True)) + '\n')


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('phase', choices=('fetch', 'package', 'print-configure', 'self-test', 'smoke-existing'))
    parser.add_argument('--package-dir', type=Path)
    parser.add_argument('--source-sha256', default='')
    parser.add_argument('--platform', choices=('windows-x64', 'linux-x64'))
    parser.add_argument('--media', type=Path, default=Path('.dev-check/youtube-media-fixture'))
    args = parser.parse_args()
    if args.phase == 'self-test': self_test(); return
    if os.environ.get('GITHUB_ACTIONS') != 'true': raise SystemExit('ci_only')
    if args.source_sha256 and not re.fullmatch('[0-9a-f]{64}', args.source_sha256): raise SystemExit('invalid_source_sha256')
    if args.phase == 'fetch': fetch(args.source_sha256)
    elif args.phase == 'print-configure': print(' '.join(CONFIGURE))
    elif not args.platform: raise SystemExit('platform_required')
    elif args.phase == 'smoke-existing':
        if not args.package_dir: raise SystemExit('package_dir_required')
        verify_bundle(args.platform, args.media, args.package_dir, existing=True)
    else: package(args.platform, args.media)

if __name__ == '__main__':
    main()
