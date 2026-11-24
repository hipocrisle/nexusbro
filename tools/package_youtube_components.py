#!/usr/bin/env python3
"""Package pinned YouTube helpers on native isolated GitHub x64 runners only.

--self-test performs tiny offline ZIP/atomicity checks and never downloads or
executes components. Normal packaging verifies each asset before opening it.
"""
import argparse
import hashlib
import json
import os
import pathlib
import platform
import re
import shutil
import stat
import subprocess
import tempfile
import threading
import time
import urllib.parse
import urllib.request
import uuid
import warnings
import zipfile

MAX_ASSET = 256 * 1024 * 1024
MAX_EXE = 256 * 1024 * 1024
MAX_LICENSE = 2 * 1024 * 1024
HOSTS = {'github.com', 'raw.githubusercontent.com', 'release-assets.githubusercontent.com', 'objects.githubusercontent.com'}


class Blocked(Exception):
    """Only fixed local classes/stages are exposed, never raw network diagnostics."""


def sha256(path):
    digest = hashlib.sha256()
    with path.open('rb') as source:
        for block in iter(lambda: source.read(1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


def allowed_url(url):
    value = urllib.parse.urlsplit(url)
    if value.scheme != 'https' or value.hostname not in HOSTS or value.username or value.password or value.port not in (None, 443):
        raise Blocked('upstream_url')


class Redirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, msg, headers, newurl):
        allowed_url(newurl)
        return super().redirect_request(request, fp, code, msg, headers, newurl)


def fetch_verified(url, expected, target, maximum):
    allowed_url(url)
    if not re.fullmatch('[0-9a-f]{64}', expected):
        raise Blocked('pin_format')
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), Redirect())
    request = urllib.request.Request(url, headers={'User-Agent': 'NexusBro-component-packaging/1', 'Accept-Encoding': 'identity'})
    digest, total, deadline = hashlib.sha256(), 0, time.monotonic() + 240
    with opener.open(request, timeout=30) as response, target.open('xb') as output:
        allowed_url(response.geturl())
        length = response.headers.get('Content-Length')
        if length and (not length.isdecimal() or int(length) > maximum):
            raise Blocked('download_size')
        for block in iter(lambda: response.read(1024 * 1024), b''):
            total += len(block)
            if total > maximum or time.monotonic() > deadline:
                raise Blocked('download_bound')
            output.write(block)
            digest.update(block)
        output.flush()
        os.fsync(output.fileno())
    if total == 0 or digest.hexdigest() != expected:
        raise Blocked('sha256')


def extract_verified_zip(archive, expected, entry, output):
    # Must execute BEFORE ZipFile parses even its central directory.
    if sha256(archive) != expected:
        raise Blocked('sha256')
    if entry not in ('deno', 'deno.exe'):
        raise Blocked('zip_entry')
    with zipfile.ZipFile(archive) as source:
        entries = source.infolist()
        if len(entries) > 32:
            raise Blocked('zip_count')
        matches = []
        for info in entries:
            path = pathlib.PurePosixPath(info.filename)
            mode = info.external_attr >> 16
            if ('\\' in info.filename or path.is_absolute() or '..' in path.parts or ':' in info.filename
                    or stat.S_ISLNK(mode) or info.flag_bits & 1):
                raise Blocked('zip_path')
            if info.filename == entry:
                matches.append(info)
        if len(matches) != 1:
            raise Blocked('zip_duplicate_or_missing')
        info = matches[0]
        if (info.is_dir() or info.file_size <= 0 or info.file_size > MAX_EXE
                or info.file_size > max(1, info.compress_size) * 200):
            raise Blocked('zip_size')
        with source.open(info) as stream, output.open('xb') as target:
            total = 0
            for block in iter(lambda: stream.read(1024 * 1024), b''):
                total += len(block)
                if total > info.file_size or total > MAX_EXE:
                    raise Blocked('zip_size')
                target.write(block)
            target.flush()
            os.fsync(target.fileno())
        if total != info.file_size:
            raise Blocked('zip_truncated')


def offline_command(executable, args, directory, limit):
    # Only the verified executable; no network arguments, inherited proxy,
    # credentials, package-manager variables or user Deno configuration.
    env = {k: os.environ[k] for k in ('SystemRoot', 'WINDIR', 'TEMP', 'TMP', 'TMPDIR') if k in os.environ}
    env.update(DENO_NO_UPDATE_CHECK='1', DENO_NO_PROMPT='1', NO_COLOR='1')
    process = subprocess.Popen([str(executable), *args], cwd=directory, env=env,
                               stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    chunks, failed = [], threading.Event()
    def reader(pipe, retain, maximum):
        total = 0
        try:
            while True:
                block = pipe.read(8192)
                if not block:
                    return
                total += len(block)
                if total > maximum:
                    failed.set()
                    process.kill()
                    return
                if retain:
                    chunks.append(block)
        except Exception:
            failed.set()
        finally:
            pipe.close()
    workers = [threading.Thread(target=reader, args=(process.stdout, True, limit)),
               threading.Thread(target=reader, args=(process.stderr, False, 65536))]
    for worker in workers:
        worker.start()
    try:
        status = process.wait(timeout=20)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)
        failed.set()
        status = -1
    finally:
        for worker in workers:
            worker.join(timeout=5)
    if failed.is_set() or any(worker.is_alive() for worker in workers) or status != 0:
        return None
    return b''.join(chunks)


def replace_directory(staged, output):
    backup = output.with_name(output.name + '.previous-' + uuid.uuid4().hex)
    moved = False
    if output.exists():
        if output.is_symlink() or not output.is_dir():
            raise Blocked('output_type')
        os.replace(output, backup)
        moved = True
    try:
        os.replace(staged, output)
    except BaseException:
        if moved:
            os.replace(backup, output)
        raise
    if moved:
        shutil.rmtree(backup)


def package(args):
    if os.environ.get('GITHUB_ACTIONS') != 'true':
        raise Blocked('github_actions_required')
    native = 'windows-x64' if os.name == 'nt' else 'linux-x64' if platform.system() == 'Linux' else None
    if platform.machine().lower() not in ('x86_64', 'amd64') or args.platform != native:
        raise Blocked('native_x64_required')
    root = pathlib.Path(__file__).resolve().parents[1]
    manifest_path = root / 'docs' / 'YOUTUBE-COMPONENTS.json'
    pins = json.loads(manifest_path.read_text())
    if pins.get('schema') != 1:
        raise Blocked('manifest_schema')
    selected = pins['platforms'][args.platform]
    output = root / 'desktop' / 'src-tauri' / 'binaries' / 'youtube-light'
    output.parent.mkdir(parents=True, exist_ok=True)
    staging = pathlib.Path(tempfile.mkdtemp(prefix='.youtube-light-', dir=output.parent))
    try:
        bundle = staging / 'bundle'
        bundle.mkdir()
        licenses = bundle / 'licenses'
        licenses.mkdir()
        records = {}
        for name in ('yt-dlp', 'deno'):
            pin = selected[name]
            asset = staging / (name + '.asset')
            fetch_verified(pin['url'], pin['sha256'], asset, MAX_ASSET)
            destination = bundle / pin['filename']
            if pin['archive']:
                extract_verified_zip(asset, pin['sha256'], pin['entry'], destination)
            else:
                os.replace(asset, destination)
            destination.chmod(0o755)
            records[name] = dict(version=pin['version'], filename=pin['filename'], sha256=sha256(destination),
                                 asset_sha256=pin['sha256'], upstream_url=pin['url'], source_url=pin['source_url'])
        license_records = []
        for pin in pins['licenses'] + pins.get('source_inventory', []):
            destination = licenses / pin['filename']
            fetch_verified(pin['url'], pin['sha256'], destination, MAX_LICENSE)
            license_records.append(dict(filename='licenses/' + pin['filename'], sha256=pin['sha256'], upstream_url=pin['url'], scope=pin['scope']))
        # Record actual versions, never execute before asset verification.
        runtime = bundle / selected['deno']['filename']
        version = offline_command(runtime, ['--version'], staging, 65536)
        if not version or not re.search(rb'^deno 2\.9\.8(?:\s|$)', version):
            raise Blocked('deno_version')
        extractor = bundle / selected['yt-dlp']['filename']
        version = offline_command(extractor, ['--ignore-config', '--no-plugin-dirs', '--version'], staging, 65536)
        if not version or version.strip() != b'2026.11.23':
            raise Blocked('extractor_version')
        # Source inspection of the exact Deno parser found no supported license
        # subcommand. Do not guess/execute one or invent an aggregate notice.
        # Matching root/rusty_v8/V8 license texts and Cargo.lock are shipped;
        # remaining transitive runtime notices require the release review.
        metadata = dict(schema=1, platform=args.platform, components=records, licenses=license_records,
                        source_manifest_sha256=sha256(manifest_path),
                        redistribution_review='pending_deno_transitive_notice_inventory',
                        deno_v8_crate='150.4.0',
                        deno_v8_revision='ac1e23989121713ca642f6650b34deff7b686896')
        (bundle / 'components.json').write_text(json.dumps(metadata, indent=2) + '\n')
        replace_directory(bundle, output)
    finally:
        shutil.rmtree(staging, ignore_errors=True)


def self_test():
    # Tiny offline fixtures only. No urllib, binaries or environment mutation.
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        archive = root / 'tiny.zip'
        with zipfile.ZipFile(archive, 'w') as z:
            z.writestr('deno', b'tiny verified fixture')
        destination = root / 'deno'
        extract_verified_zip(archive, sha256(archive), 'deno', destination)
        assert destination.read_bytes() == b'tiny verified fixture'
        invalid = root / 'invalid.zip'
        invalid.write_bytes(b'not a zip')
        try:
            extract_verified_zip(invalid, '0' * 64, 'deno', root / 'bad')
            raise AssertionError('unverified archive opened')
        except Blocked as error:
            assert str(error) == 'sha256'
        for n, entries in enumerate([[('../escape', b'x'), ('deno', b'x')], [('deno', b'x'), ('deno', b'y')], [('deno.exe', b'x')]]):
            bad = root / ('bad' + str(n) + '.zip')
            with warnings.catch_warnings():
                warnings.simplefilter('ignore', UserWarning)
                with zipfile.ZipFile(bad, 'w') as z:
                    for name, data in entries:
                        z.writestr(name, data)
            try:
                extract_verified_zip(bad, sha256(bad), 'deno', root / ('output' + str(n)))
                raise AssertionError('unsafe archive accepted')
            except Blocked:
                pass
        symlink = root / 'symlink.zip'
        with zipfile.ZipFile(symlink, 'w') as z:
            info = zipfile.ZipInfo('deno')
            info.create_system = 3
            info.external_attr = (stat.S_IFLNK | 0o777) << 16
            z.writestr(info, b'outside')
        try:
            extract_verified_zip(symlink, sha256(symlink), 'deno', root / 'symlink-out')
            raise AssertionError('symlink accepted')
        except Blocked as error:
            assert str(error) == 'zip_path'
        old, new = root / 'old', root / 'new'
        old.mkdir(); new.mkdir()
        (old / 'proof').write_text('old'); (new / 'proof').write_text('new')
        replace_directory(new, old)
        assert (old / 'proof').read_text() == 'new'
        replacement = root / 'replacement'
        replacement.mkdir()
        (replacement / 'proof').write_text('uncommitted')
        original_replace = os.replace
        def injected(source, target):
            if pathlib.Path(source) == replacement:
                raise OSError('injected rename failure')
            return original_replace(source, target)
        try:
            os.replace = injected
            try:
                replace_directory(replacement, old)
                raise AssertionError('failed replacement accepted')
            except OSError:
                pass
        finally:
            os.replace = original_replace
        assert (old / 'proof').read_text() == 'new'
    print(json.dumps({'self_test': True, 'network': False, 'executed_components': False}))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--platform', choices=('windows-x64', 'linux-x64'))
    parser.add_argument('--self-test', action='store_true')
    args = parser.parse_args()
    try:
        if args.self_test:
            self_test()
        else:
            package(args)
            print(json.dumps({'status': 'PACKAGED'}))
    except Exception as error:
        print(json.dumps({'status': 'BLOCKED', 'stage': str(error) if isinstance(error, Blocked) else 'packaging_failure', 'class': type(error).__name__}))
        raise SystemExit(2)
