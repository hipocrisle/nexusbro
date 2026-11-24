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
import shlex
import stat
import tarfile
import posixpath
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
MAX_FFMPEG_EXPANDED = 512 * 1024 * 1024
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


def offline_command(executable, args, directory, limit, extra_env=None, merge_stderr=False):
    # Only the verified executable; no network arguments, inherited proxy,
    # credentials, package-manager variables or user Deno configuration.
    env = {k: os.environ[k] for k in ('SystemRoot', 'WINDIR', 'TEMP', 'TMP', 'TMPDIR') if k in os.environ}
    env.update(DENO_NO_UPDATE_CHECK='1', DENO_NO_PROMPT='1', NO_COLOR='1')
    if extra_env:
        env.update(extra_env)
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
               threading.Thread(target=reader, args=(process.stderr, merge_stderr, limit if merge_stderr else 65536))]
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


def extract_ffmpeg(archive, pin, output, native_platform, expanded_limit=MAX_FFMPEG_EXPANDED):
    """Verify the whole archive before parsing; materialize a closed file graph."""
    if sha256(archive) != pin['sha256']:
        raise Blocked('ffmpeg_sha256')
    if not 0 < expanded_limit <= MAX_FFMPEG_EXPANDED:
        raise Blocked('ffmpeg_expanded_size')
    expected_root = pin['root']
    source = zipfile.ZipFile(archive) if pin['format'] == 'zip' else tarfile.open(archive, mode='r:xz')
    try:
        if pin['format'] == 'zip':
            entries = source.infolist()
        else:
            entries = []
            for info in source:
                if len(entries) >= 4096:
                    raise Blocked('ffmpeg_entries')
                entries.append(info)
        if len(entries) > 4096:
            raise Blocked('ffmpeg_entries')
        files, links, seen, expanded = {}, {}, set(), 0
        for info in entries:
            name = info.filename if pin['format'] == 'zip' else info.name
            path = pathlib.PurePosixPath(name)
            if ('\\' in name or ':' in name or any(ord(c) < 32 or ord(c) == 127 for c in name) or path.is_absolute() or '..' in path.parts
                    or not path.parts or path.parts[0] != expected_root or len(name) > 512):
                raise Blocked('ffmpeg_path')
            relative = pathlib.PurePosixPath(*path.parts[1:]).as_posix()
            identity = relative.casefold()
            if identity in seen:
                raise Blocked('ffmpeg_duplicate')
            seen.add(identity)
            directory = info.is_dir() if pin['format'] == 'zip' else info.isdir()
            if directory:
                continue
            size = info.file_size if pin['format'] == 'zip' else info.size
            expanded += size
            if size < 0 or size > MAX_EXE or expanded > expanded_limit:
                raise Blocked('ffmpeg_expanded_size')
            if pin['format'] == 'zip':
                mode = info.external_attr >> 16
                if stat.S_ISLNK(mode) or info.flag_bits & 1:
                    raise Blocked('ffmpeg_zip_link')
                files[relative] = info
            elif info.issym():
                target = info.linkname
                if '\\' in target or ':' in target or len(target) > 512 or pathlib.PurePosixPath(target).is_absolute():
                    raise Blocked('ffmpeg_link')
                resolved = posixpath.normpath(posixpath.join(posixpath.dirname(relative), target))
                if not relative.startswith('lib/') or not resolved.startswith('lib/') or '..' in pathlib.PurePosixPath(resolved).parts:
                    raise Blocked('ffmpeg_link_escape')
                links[relative] = resolved
            elif info.isfile():
                files[relative] = info
            else:
                raise Blocked('ffmpeg_special_entry')
        def source_allowed(name):
            path = pathlib.PurePosixPath(name)
            if name == 'LICENSE.txt':
                return True
            if native_platform == 'windows-x64':
                return len(path.parts) == 2 and path.parts[0] == 'bin' and (path.name.lower() in ('ffmpeg.exe', 'ffprobe.exe') or path.suffix.lower() == '.dll')
            return name in ('bin/ffmpeg', 'bin/ffprobe') or (len(path.parts) == 2 and path.parts[0] == 'lib' and re.fullmatch(r'[A-Za-z0-9_+.-]+\.so(?:\.[0-9]+)*', path.name))
        def resolve(name):
            origin, visited = name, set()
            while origin in links:
                if origin in visited or len(visited) >= 16:
                    raise Blocked('ffmpeg_link_cycle')
                visited.add(origin)
                origin = links[origin]
            if origin not in files or not source_allowed(origin):
                raise Blocked('ffmpeg_link_target')
            return origin
        candidates = [name for name in [*files, *links] if source_allowed(name)]
        # Validate aliases even when they will not be installed. Source nodes
        # and runtime output names are deliberately different sets.
        for name in links:
            if not source_allowed(name):
                raise Blocked('ffmpeg_link_target')
            resolve(name)
        targets = candidates
        if native_platform == 'linux-x64':
            families = {}
            for name in candidates:
                if not name.startswith('lib/'):
                    continue
                match = re.fullmatch(r'(lib/[A-Za-z0-9_+.-]+\.so)(?:\.([0-9]+)(?:\.[0-9]+)*)?', name)
                if not match:
                    raise Blocked('ffmpeg_library_name')
                families.setdefault(match[1], []).append((name, match[2]))
            selected_libraries = []
            for family, members in families.items():
                majors = {major for _, major in members if major is not None}
                if len(majors) != 1:
                    raise Blocked('ffmpeg_ambiguous_or_missing_soname')
                major = next(iter(majors))
                soname = family + '.' + major
                if soname not in candidates:
                    raise Blocked('ffmpeg_missing_soname_alias')
                origin = resolve(soname)
                if not re.fullmatch(re.escape(family) + r'\.' + re.escape(major) + r'(?:\.[0-9]+)*', origin):
                    raise Blocked('ffmpeg_soname_target_family')
                selected_libraries.append(soname)
            targets = [name for name in candidates if not name.startswith('lib/')] + selected_libraries
        expected = {'LICENSE.txt', 'bin/ffmpeg.exe', 'bin/ffprobe.exe'} if native_platform == 'windows-x64' else {'LICENSE.txt', 'bin/ffmpeg', 'bin/ffprobe'}
        if not expected.issubset(targets) or len(targets) > 256:
            raise Blocked('ffmpeg_required_files')
        has_shared = (any(name.lower().endswith('.dll') for name in targets) if native_platform == 'windows-x64'
                      else any(name.startswith('lib/') for name in targets))
        if not has_shared:
            raise Blocked('ffmpeg_missing_shared_libraries')
        output.mkdir()
        records, written = [], 0
        for name in sorted(targets):
            origin = resolve(name)
            destination = output / pathlib.PurePosixPath(name)
            destination.parent.mkdir(parents=True, exist_ok=True)
            info = files[origin]
            stream = source.open(info) if pin['format'] == 'zip' else source.extractfile(info)
            if stream is None:
                raise Blocked('ffmpeg_stream')
            count = 0
            with stream, destination.open('xb') as target:
                for block in iter(lambda: stream.read(1024 * 1024), b''):
                    count += len(block)
                    written += len(block)
                    if count > MAX_EXE or written > expanded_limit:
                        raise Blocked('ffmpeg_materialized_size')
                    target.write(block)
                target.flush()
                os.fsync(target.fileno())
            expected_size = info.file_size if pin['format'] == 'zip' else info.size
            if count != expected_size:
                raise Blocked('ffmpeg_truncated')
            destination.chmod(0o755 if name.startswith('bin/') else 0o644)
            records.append(dict(filename='ffmpeg/' + name, sha256=sha256(destination), bytes=count,
                                materialized_from=origin if origin != name else None))
        return records
    finally:
        source.close()


def verify_ffmpeg(output, pin, native_platform, staging):
    exe = output / 'bin' / ('ffmpeg.exe' if native_platform == 'windows-x64' else 'ffmpeg')
    probe = output / 'bin' / ('ffprobe.exe' if native_platform == 'windows-x64' else 'ffprobe')
    private_env = {'PATH': str(output / 'bin')} if native_platform == 'windows-x64' else {'LD_LIBRARY_PATH': str(output / 'lib')}
    # All invocations are offline metadata checks after whole archive hashing.
    version = offline_command(exe, ['-version'], staging, 1024 * 1024, private_env, merge_stderr=True)
    probe_version = offline_command(probe, ['-version'], staging, 1024 * 1024, private_env, merge_stderr=True)
    config = offline_command(exe, ['-buildconf'], staging, 1024 * 1024, private_env, merge_stderr=True)
    license_text = offline_command(exe, ['-L'], staging, 1024 * 1024, private_env, merge_stderr=True)
    if not all((version, probe_version, config, license_text)):
        raise Blocked('ffmpeg_verification_failed')
    expected = pin['version'].encode()
    if expected not in version or expected not in probe_version:
        raise Blocked('ffmpeg_version_mismatch')
    if (b'--enable-shared' not in config or b'--enable-version3' not in config
            or any(flag in config for flag in (b'--enable-gpl', b'--enable-nonfree', b'--enable-libx264', b'--enable-libx265', b'--enable-libfdk-aac'))):
        raise Blocked('ffmpeg_license_configuration')
    if b'GNU Lesser General Public License' not in license_text or not (b'version 3' in license_text.lower() or b'version 3' in license_text):
        raise Blocked('ffmpeg_license_mismatch')
    for label, contents in [('version', version), ('probe-version', probe_version), ('buildconf', config), ('license', license_text)]:
        (output / ('verified-' + label + '.txt')).write_bytes(contents)
    return dict(shared=True, enable_version3=True, enable_gpl=False, enable_nonfree=False,
                actual_license='LGPL-3.0-or-later', dependency_closure='bundled_bin_dll_or_lib_so_native_startup_verified_os_dependencies_not_certified')

def validate_muxer_package(directory, pin, native_platform):
    directory = pathlib.Path(directory)
    if directory.is_symlink(): raise Blocked('muxer_symlink')
    manifest = directory / 'components.json'
    if manifest.is_symlink() or not manifest.is_file() or manifest.stat().st_size > 131072:
        raise Blocked('muxer_manifest')
    try: record = json.loads(manifest.read_text())['ffmpeg']
    except (ValueError, KeyError, TypeError): raise Blocked('muxer_manifest') from None
    if (record.get('platform') != native_platform or record.get('source_commit') != pin['source_commit']
            or record.get('source_sha256') != pin['source_sha256']
            or record.get('source_url') != pin['source_archive_url']
            or record.get('configure') != pin['configure'] or record.get('license') != 'LGPL-2.1-or-later'
            or record.get('source_pin_verified') is not True or record.get('bootstrap_only') is not False
            or record.get('native_smoke') is not True or record.get('network_protocol_refused') is not True):
        raise Blocked('muxer_source_or_evidence')
    files = record.get('files')
    if not isinstance(files, list) or len(files) != len(pin['files']): raise Blocked('muxer_files')
    seen, total = set(), 0
    for item in files:
        if not isinstance(item, dict): raise Blocked('muxer_files')
        name, size, digest = item.get('filename'), item.get('bytes'), item.get('sha256')
        if (name not in pin['files'] or name in seen or not isinstance(digest, str)
                or not re.fullmatch('[0-9a-f]{64}', digest) or type(size) is not int or not 0 < size <= MAX_EXE):
            raise Blocked('muxer_files')
        path = directory
        for part in pathlib.PurePosixPath(name).parts:
            path = path / part
            if path.is_symlink(): raise Blocked('muxer_symlink')
        total += size
        if total > MAX_FFMPEG_EXPANDED or not path.is_file() or path.stat().st_size != size or sha256(path) != digest:
            raise Blocked('muxer_file_hash')
        seen.add(name)
    actual = set()
    for index, path in enumerate((directory / 'ffmpeg').rglob('*')):
        if index >= 64: raise Blocked('muxer_file_closure')
        if path.is_symlink(): raise Blocked('muxer_symlink')
        if path.is_file(): actual.add(path.relative_to(directory).as_posix())
        elif not path.is_dir(): raise Blocked('muxer_special_file')
    executable = 'ffmpeg/bin/ffmpeg' + ('.exe' if native_platform == 'windows-x64' else '')
    probe = 'ffmpeg/bin/ffprobe' + ('.exe' if native_platform == 'windows-x64' else '')
    if actual != seen or seen != set(pin['files']) or record.get('filename') != executable or record.get('ffprobe_filename') != probe:
        raise Blocked('muxer_file_closure')
    if record.get('sha256') != next(item['sha256'] for item in files if item['filename'] == executable):
        raise Blocked('muxer_executable_hash')
    verification = record.get('verification')
    if not isinstance(verification, dict) or not re.fullmatch('[0-9a-f]{64}', str(verification.get('-version', ''))):
        raise Blocked('muxer_version_evidence')
    return record


def package_minimal_muxer(directory, pin, native_platform, bundle, staging):
    record = validate_muxer_package(directory, pin, native_platform)  # BEFORE copy/EXEC
    output = bundle / 'ffmpeg'
    output.mkdir()
    for item in record['files']:
        relative = pathlib.PurePosixPath(item['filename']).relative_to('ffmpeg')
        target = output.joinpath(*relative.parts); target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(pathlib.Path(directory) / item['filename'], target)
        if sha256(target) != item['sha256']: raise Blocked('muxer_copy_hash')
    exe = output / 'bin' / ('ffmpeg.exe' if native_platform == 'windows-x64' else 'ffmpeg')
    probe = output / 'bin' / ('ffprobe.exe' if native_platform == 'windows-x64' else 'ffprobe')
    if native_platform == 'linux-x64':
        for path in (exe, probe): path.chmod(0o700)
    env = {'PATH': str(output/'bin')} if native_platform == 'windows-x64' else {'LD_LIBRARY_PATH': str(output/'lib')}
    results = {}
    for label, binary, option in [('version',exe,'-version'),('probe-version',probe,'-version'),('buildconf',exe,'-buildconf'),('license',exe,'-L')]:
        data = offline_command(binary, [option], staging, 1024*1024, env, merge_stderr=True)
        if not data: raise Blocked('muxer_native_verification')
        results[label] = data
    if hashlib.sha256(results['version']).hexdigest() != record['verification']['-version']:
        raise Blocked('muxer_version_evidence')
    match = re.match(rb'ffmpeg version ([A-Za-z0-9._+-]{1,80})(?:\s|$)', results['version'])
    if not match or not results['probe-version'].startswith(b'ffprobe version '+match[1]+b' '): raise Blocked('muxer_version')
    options = [word for word in shlex.split(results['buildconf'].decode('utf-8','strict')) if word.startswith('--')]
    if any(flag not in options for flag in pin['configure']) or any(flag not in pin['configure'] and not flag.startswith('--prefix=') and flag not in ('--toolchain=msvc','--ld=link.exe') for flag in options):
        raise Blocked('muxer_native_configuration')
    text = b' '.join(results['license'].split())
    if b'GNU Lesser General Public License' not in text or b'version 2.1 of the License' not in text:
        raise Blocked('muxer_native_license')
    for label, data in results.items(): (output/('verified-'+label+'.txt')).write_bytes(data)
    files = [dict(filename='ffmpeg/'+path.relative_to(output).as_posix(),sha256=sha256(path),bytes=path.stat().st_size) for path in sorted(output.rglob('*')) if path.is_file()]
    return dict(version=match[1].decode('ascii'),filename=record['filename'],ffprobe_filename=record['ffprobe_filename'],sha256=sha256(exe),files=files,
                source_url=pin['source_url'],source_archive_url=pin['source_archive_url'],source_archive_sha256=pin['source_sha256'],source_commit=pin['source_commit'],
                recipe_url=pin['recipe_url'],recipe_provenance=pin['recipe_provenance'],license='LGPL-2.1-or-later',verification=dict(source_built=True,native_metadata=True,upstream_native_smoke=True,network_protocol_refused=True),
                redistribution_review='preserve_exact_source_recipe_and_license_closure_release_review_pending')


def package(args):
    if bool(args.extractor_only) == bool(args.muxer_package):
        raise Blocked('muxer_package_required_or_extractor_only')
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
        if not args.extractor_only:
            records['ffmpeg'] = package_minimal_muxer(args.muxer_package, selected['ffmpeg'], args.platform, bundle, staging)
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


def ffmpeg_self_tests(root):
    import io
    pin = dict(root='fixture', format='zip')
    archive = root / 'ffmpeg.zip'
    with zipfile.ZipFile(archive, 'w') as z:
        for name, data in [('bin/ffmpeg.exe', b'ffmpeg'), ('bin/ffprobe.exe', b'ffprobe'), ('bin/avcodec.dll', b'dll'), ('LICENSE.txt', b'license')]:
            z.writestr('fixture/' + name, data)
    pin['sha256'] = sha256(archive)
    output = root / 'ffmpeg-windows'
    records = extract_ffmpeg(archive, pin, output, 'windows-x64')
    assert len(records) == 4 and (output / 'bin/avcodec.dll').read_bytes() == b'dll'
    archive = root / 'ffmpeg.tar.xz'
    with tarfile.open(archive, 'w:xz') as source:
        for name, data in [('bin/ffmpeg', b'ffmpeg'), ('bin/ffprobe', b'ffprobe'), ('lib/libavcodec.so.1.2.3', b'x' * 20000), ('LICENSE.txt', b'license')]:
            info = tarfile.TarInfo('fixture/' + name); info.size = len(data)
            source.addfile(info, io.BytesIO(data))
        info = tarfile.TarInfo('fixture/lib/libavcodec.so.1'); info.type = tarfile.SYMTYPE; info.linkname = 'libavcodec.so.1.2.3'
        source.addfile(info)
        info = tarfile.TarInfo('fixture/lib/libavcodec.so'); info.type = tarfile.SYMTYPE; info.linkname = 'libavcodec.so.1'
        source.addfile(info)
    pin = dict(root='fixture', format='tar.xz', sha256=sha256(archive))
    output = root / 'ffmpeg-linux'
    # Source bytes fit 30KiB, but three physical alias copies would exceed it.
    records = extract_ffmpeg(archive, pin, output, 'linux-x64', expanded_limit=30000)
    assert len(records) == 4 and not (output / 'lib/libavcodec.so.1').is_symlink()
    assert (output / 'lib/libavcodec.so.1').read_bytes() == b'x' * 20000
    assert not (output / 'lib/libavcodec.so').exists()
    assert not (output / 'lib/libavcodec.so.1.2.3').exists()
    assert any(r['materialized_from'] == 'lib/libavcodec.so.1.2.3' for r in records)
    assert sum(r['bytes'] for r in records) < 30000
    for name, members in [('missing-major', ['lib/sample.so.1.2']),
                          ('unversioned-only', ['lib/sample.so']),
                          ('ambiguous', ['lib/sample.so.1', 'lib/sample.so.2'])]:
        bad = root / ('ffmpeg-' + name + '.tar.xz')
        with tarfile.open(bad, 'w:xz') as source:
            for member in ['bin/ffmpeg', 'bin/ffprobe', 'LICENSE.txt', *members]:
                info = tarfile.TarInfo('fixture/' + member); info.size = 1
                source.addfile(info, io.BytesIO(b'x'))
        try:
            extract_ffmpeg(bad, dict(root='fixture', format='tar.xz', sha256=sha256(bad)), root / name, 'linux-x64')
            raise AssertionError('missing/ambiguous ABI alias accepted')
        except Blocked:
            pass
    for case in ('traversal', 'escape', 'cycle', 'hardlink'):
        bad = root / ('ffmpeg-' + case + '.tar.xz')
        with tarfile.open(bad, 'w:xz') as source:
            for name in ['bin/ffmpeg', 'bin/ffprobe', 'LICENSE.txt', 'lib/ok.so']:
                info = tarfile.TarInfo('fixture/' + name); info.size = 1
                source.addfile(info, io.BytesIO(b'x'))
            if case == 'traversal':
                info = tarfile.TarInfo('fixture/../outside'); info.size = 1; source.addfile(info, io.BytesIO(b'x'))
            else:
                info = tarfile.TarInfo('fixture/lib/evil.so')
                info.type = tarfile.LNKTYPE if case == 'hardlink' else tarfile.SYMTYPE
                info.linkname = '../../outside' if case == 'escape' else 'evil.so'
                source.addfile(info)
        bad_pin = dict(root='fixture', format='tar.xz', sha256=sha256(bad))
        try:
            extract_ffmpeg(bad, bad_pin, root / ('reject-' + case), 'linux-x64')
            raise AssertionError('unsafe FFmpeg TAR accepted')
        except Blocked:
            pass
    corrupt = root / 'not-ffmpeg.tar.xz'; corrupt.write_bytes(b'not an archive')
    try:
        extract_ffmpeg(corrupt, dict(root='fixture', format='tar.xz', sha256='0'*64), root/'never', 'linux-x64')
        raise AssertionError('unverified TAR parsed')
    except Blocked as error:
        assert str(error) == 'ffmpeg_sha256'


def minimal_muxer_self_tests(root):
    pins=json.loads((pathlib.Path(__file__).resolve().parents[1]/'docs/YOUTUBE-COMPONENTS.json').read_text())
    for platform_name in ('linux-x64','windows-x64'):
        pin=pins['platforms'][platform_name]['ffmpeg']; directory=root/platform_name; directory.mkdir()
        files=[]
        for name in pin['files']:
            path=directory/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_bytes(b'tiny fixture')
            files.append(dict(filename=name,bytes=path.stat().st_size,sha256=sha256(path)))
        record=dict(platform=platform_name,source_commit=pin['source_commit'],source_sha256=pin['source_sha256'],source_url=pin['source_archive_url'],configure=pin['configure'],license=pin['license'],native_smoke=True,network_protocol_refused=True,source_pin_verified=True,bootstrap_only=False,files=files,filename='ffmpeg/bin/ffmpeg'+('.exe' if platform_name=='windows-x64' else ''),ffprobe_filename='ffmpeg/bin/ffprobe'+('.exe' if platform_name=='windows-x64' else ''),verification={'-version':'0'*64})
        record['sha256']=next(item['sha256'] for item in files if item['filename']==record['filename'])
        manifest=directory/'components.json'
        def write(value):manifest.write_text(json.dumps({'ffmpeg':value}))
        def refused(value):
            write(value)
            try:validate_muxer_package(directory,pin,platform_name)
            except Blocked:return
            raise AssertionError('invalid minimal muxer accepted')
        write(record);assert validate_muxer_package(directory,pin,platform_name)['native_smoke']
        for field,value in [('source_sha256','0'*64),('native_smoke',False),('network_protocol_refused',False),('source_pin_verified',False),('bootstrap_only',True),('configure',pin['configure'][:-1])]:
            changed=dict(record);changed[field]=value;refused(changed)
        changed=json.loads(json.dumps(record));changed['files'][0]['filename']='../escape';refused(changed)
        changed=json.loads(json.dumps(record));changed['files'][0]['sha256']='0'*64;refused(changed)
        path=directory/files[0]['filename'];old=path.read_bytes();path.write_bytes(b'tampered');refused(record);path.write_bytes(old)
        path.unlink();refused(record);path.write_bytes(old)
        extra=directory/'ffmpeg'/'unlisted';extra.write_bytes(b'extra');refused(record);extra.unlink()
        write(record);assert validate_muxer_package(directory,pin,platform_name)['files']==files


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
        ffmpeg_self_tests(root)
        minimal_muxer_self_tests(root)
    print(json.dumps({'self_test': True, 'network': False, 'executed_components': False}))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--platform', choices=('windows-x64', 'linux-x64'))
    parser.add_argument('--self-test', action='store_true')
    parser.add_argument('--muxer-package', type=pathlib.Path, help='Trusted same-run minimal muxer artifact package directory; required for desktop')
    parser.add_argument('--extractor-only', action='store_true', help='Isolated extractor probe only; desktop bundles always include the muxer')
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
