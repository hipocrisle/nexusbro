#!/usr/bin/env python3
"""GHA-only diagnostic: Deno cache fallback observations, not a sandbox proof.
Only safe numeric/bool aggregates leave the owned private temporary directory.
"""
import argparse
import ast
import hashlib
import json
import os
from pathlib import Path
import re
import resource
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time

FLAGS = ['run', '--ext=js', '--no-code-cache', '--no-prompt', '--no-remote',
         '--no-lock', '--node-modules-dir=none', '--no-config', '--no-npm', '--cached-only', '-']
# Exact yt-dlp2026.11.23 non-NPM Deno JSC invocation; no unsafe TLS/jit override.
FLAGS_SOURCE = 'https://raw.githubusercontent.com/yt-dlp/yt-dlp/6f73a8d1664a58cdef8c25f01ffb5ab9cf730408/yt_dlp/extractor/youtube/jsc/_builtin/deno.py'
MARKER = b'NB_DENO_CACHE_PROBE_V1\n'
SCRIPT = b'let n=0; for(let i=0;i<2000;i++){n+=(/^[a-z]+$/u.test("cache")?i:0);} if(n!==1999000)throw Error("canary"); console.log("NB_DENO_CACHE_PROBE_V1");\n'
TRACE_LIMIT = 32 * 1024 * 1024
WRITE_CALLS = {'mkdir', 'mkdirat', 'unlink', 'unlinkat', 'rmdir', 'rename', 'renameat', 'renameat2',
               'link', 'linkat', 'symlink', 'symlinkat', 'chmod', 'fchmodat', 'chown', 'lchown',
               'fchownat', 'truncate', 'utime', 'utimes', 'utimensat', 'mknod', 'mknodat'}
FD_WRITE_CALLS = {'write', 'writev', 'pwrite64', 'pwritev', 'pwritev2', 'ftruncate'}
QUOTED = re.compile(r'"(?:[^"\\]|\\.)*"')
ANNOTATED = re.compile(r'\d+<([^>]+)>')
CALL = re.compile(r'^(?:\[pid\s+\d+\]\s+|\d+\s+)?([A-Za-z0-9_]+)\((.*)\)\s+=\s+(.+)$')


SAFE_REASONS = {
    'strace_missing', 'native_output_bound', 'trace_bound', 'cache_file_bound',
    'cache_symlink_not_followed', 'positive_scanner_canary_failed',
    'case_permission_precondition_failed', 'pinned_component_verification_failed',
}
SAFE_DETAILS = {'exit_code', 'marker', 'stdout_bytes', 'stderr_bytes', 'trace_bytes',
                'trace_lines', 'elapsed_ms', 'attempt', 'completed_attempts'}


class ProbeFailure(Exception):
    def __init__(self, step, reason, details=None, errno=None):
        super().__init__(reason)
        self.step, self.reason = step, reason
        self.details = dict(details or {})
        self.errno = errno if type(errno) is int and 0 <= errno <= 4096 else None

    def public(self):
        return dict(step=self.step, reason=self.reason, errno=self.errno,
                    **{key: value for key, value in self.details.items()
                       if key in SAFE_DETAILS and type(value) in (int, bool)})


def run_step(step, action, details=None):
    try: return action()
    except (ProbeFailure, subprocess.TimeoutExpired): raise
    except (ValueError, KeyError, OSError, subprocess.SubprocessError) as error:
        # Only exact in-source literals are reflected. str(error) is used solely
        # for whitelist membership and is never logged, retained or serialized.
        reason = str(error) if type(error) is ValueError and str(error) in SAFE_REASONS else (
            'os_error' if isinstance(error, OSError) else
            'process_setup_error' if isinstance(error, subprocess.SubprocessError) else
            'missing_metadata' if isinstance(error, KeyError) else 'unclassified_value_error')
        raise ProbeFailure(step, reason, details, getattr(error, 'errno', None)) from None


def digest(path):
    h = hashlib.sha256()
    with path.open('rb') as f:
        for block in iter(lambda: f.read(1024 * 1024), b''): h.update(block)
    return h.hexdigest()


def contained(path, root):
    try: Path(path).relative_to(root); return True
    except ValueError: return False


def decoded_string(token):
    try:
        value = ast.literal_eval(token)
        return value if isinstance(value, str) and '\x00' not in value else None
    except (ValueError, SyntaxError): return None


READ_CALLS = {'access', 'faccessat', 'faccessat2', 'stat', 'lstat', 'fstat', 'newfstatat',
              'statx', 'statfs', 'fstatfs', 'readlink', 'readlinkat', 'getcwd', 'execve', 'execveat'}
OPEN_CALLS = {'open', 'openat', 'openat2', 'creat'}
FD_WRITE_CALLS = FD_WRITE_CALLS | {'fchmod', 'fchown'}
WRITE_CALLS = WRITE_CALLS | {'fchmodat2'}
SAFE_SYSCALLS = READ_CALLS | OPEN_CALLS | WRITE_CALLS | FD_WRITE_CALLS | {'mmap', 'chdir', 'fchdir'}
PREFIX = re.compile(r'^(?:\[pid\s+(\d+)\]\s+|(\d+)\s+)?(.*)$')
RESUMED = re.compile(r'^<\.\.\. ([A-Za-z0-9_]+) resumed>(.*)$')


def syscall_args(text):
    result, start, depth, quoted, escaped = [], 0, 0, False, False
    for i, char in enumerate(text):
        if quoted:
            if escaped: escaped = False
            elif char == '\\': escaped = True
            elif char == '"': quoted = False
        elif char == '"': quoted = True
        elif char in '([{': depth += 1
        elif char in ')]}': depth -= 1
        elif char == ',' and depth == 0:
            result.append(text[start:i].strip()); start = i + 1
    if quoted or depth != 0: raise ValueError('incomplete_syscall_args')
    result.append(text[start:].strip())
    return result


def scan_trace(data, allowed, cwd, cache_root, existed=()):
    """Parse pathname operands, not arbitrary quoted data; unknown writes fail closed."""
    stats = dict(write_attempts=0, successful_writes=0, denied_writes=0,
                 outside_write_attempts=0, outside_successful_writes=0,
                 cache_write_attempts=0, cache_read_syscalls=0, cache_read_hits=0,
                 unresolved_file_operations=0, unresolved_read_operations=0,
                 unresolved_mutating_operations=0, unresolved_state_operations=0,
                 unresolved_syscalls={}, reassembled_operations=0, enotdir_events=0, eacces_events=0)
    known = {str(p) for p in existed}
    pending = {}
    current_cwd = str(cwd)
    cwd_known = True

    def unresolved(call, category=None):
        stats['unresolved_file_operations'] += 1
        bucket = call if call in SAFE_SYSCALLS else 'other'
        counts = stats['unresolved_syscalls']; counts[bucket] = counts.get(bucket, 0) + 1
        if category is None:
            category = 'read' if call in READ_CALLS else 'state' if call in ('chdir', 'fchdir') else 'mutating'
        stats['unresolved_' + category + '_operations'] += 1

    def fd_path(token):
        match = re.search(r'<([^>]+)>', token)
        if not match: return None
        raw = match[1].removesuffix(' (deleted)')
        if raw.startswith(('pipe:', 'socket:', 'anon_inode:', '/memfd:', '/anon_inode:')): return ''
        return os.path.normpath(raw) if raw.startswith('/') else None

    def pathname(token, directory=None):
        raw = decoded_string(token)
        if raw is None: return None
        if raw.startswith('/'): return os.path.normpath(raw)
        base = current_cwd if cwd_known else None
        if directory is not None:
            if directory.startswith('AT_FDCWD'):
                annotated = fd_path(directory)
                if annotated: base = annotated
            else: base = fd_path(directory)
        return os.path.normpath(os.path.join(base, raw)) if base else None

    def complete(line):
        nonlocal current_cwd, cwd_known
        match = CALL.match(line)
        if not match:
            call = re.match(r'^([A-Za-z0-9_]+)\(', line)
            if call: unresolved(call[1])
            return
        call, arguments, result = match.groups()
        if call in READ_CALLS: return
        success = not result.startswith('-1')
        try: args = syscall_args(arguments)
        except ValueError: unresolved(call); return
        if call in ('chdir', 'fchdir'):
            if success:
                path = pathname(args[0]) if call == 'chdir' else fd_path(args[0])
                if path: current_cwd, cwd_known = path, True
                else: cwd_known = False; unresolved(call)
            return
        writing = True
        cache_read = False
        paths = []
        try:
            if call in OPEN_CALLS:
                at = call in ('openat', 'openat2')
                path = pathname(args[1] if at else args[0], args[0] if at else None)
                flags = args[2] if at else args[1]
                writing = call == 'creat' or any(flag in flags for flag in ('O_WRONLY', 'O_RDWR', 'O_CREAT', 'O_TRUNC', 'O_TMPFILE'))
                cache_read = call != 'creat' and 'O_WRONLY' not in flags
                paths = [path]
                if success:
                    resolved = fd_path(result)
                    if resolved is not None: paths.append(resolved)
            elif call == 'symlink':
                # target text is DATA. Only the new link pathname is mutated.
                paths = [pathname(args[1])]
            elif call == 'symlinkat':
                paths = [pathname(args[2], args[1])]
            elif call in ('renameat', 'renameat2', 'linkat'):
                paths = [pathname(args[1], args[0]), pathname(args[3], args[2])]
            elif call in ('rename', 'link'):
                paths = [pathname(args[0]), pathname(args[1])]
            elif call in ('mkdirat', 'unlinkat', 'fchmodat', 'fchmodat2', 'fchownat', 'utimensat', 'mknodat'):
                paths = [pathname(args[1], args[0])]
            elif call in WRITE_CALLS:
                paths = [pathname(args[0])]
            elif call in FD_WRITE_CALLS:
                paths = [fd_path(args[0])]
            elif call == 'mmap':
                if 'PROT_WRITE' not in args[2] or 'MAP_SHARED' not in args[3]: return
                if 'MAP_ANONYMOUS' in args[3] and args[4] == '-1': return
                paths = [fd_path(args[4])]
            else:
                unresolved(call); return
        except IndexError:
            unresolved(call); return
        if any(path is None for path in paths):
            unresolved(call, 'mutating' if writing else 'read')
        for raw in {path for path in paths if path}:
            path = Path(raw)
            if writing:
                stats['write_attempts'] += 1
                stats['successful_writes' if success else 'denied_writes'] += 1
                if not contained(path, allowed):
                    stats['outside_write_attempts'] += 1
                    if success: stats['outside_successful_writes'] += 1
                if contained(path, cache_root): stats['cache_write_attempts'] += 1
            if cache_read and success and contained(path, cache_root):
                stats['cache_read_syscalls'] += 1
                if str(path) in known: stats['cache_read_hits'] += 1

    for line in data.splitlines():
        if 'ENOTDIR' in line: stats['enotdir_events'] += 1
        if 'EACCES' in line: stats['eacces_events'] += 1
        prefix = PREFIX.match(line)
        if not prefix: continue
        pid, body = prefix[1] or prefix[2] or 'main', prefix[3]
        resumed = RESUMED.match(body)
        if resumed:
            previous = pending.pop(pid, None)
            if previous and previous[0] == resumed[1]:
                stats['reassembled_operations'] += 1
                complete(previous[1] + resumed[2])
            else:
                if previous: unresolved(previous[0])
                unresolved(resumed[1])
            continue
        if '<unfinished ...>' in body:
            call = re.match(r'^([A-Za-z0-9_]+)\(', body)
            if call:
                previous = pending.pop(pid, None)
                if previous: unresolved(previous[0])
                pending[pid] = (call[1], body.split('<unfinished ...>', 1)[0])
            continue
        complete(body)
    for call, _ in pending.values(): unresolved(call)
    return stats


def child_limits():
    resource.setrlimit(resource.RLIMIT_FSIZE, (TRACE_LIMIT, TRACE_LIMIT))
    resource.setrlimit(resource.RLIMIT_CPU, (20, 20))
    os.umask(0o077)


def traced(command, env, cwd, trace, stdin, details=None):
    details = details if details is not None else {}
    started = time.monotonic()
    strace = shutil.which('strace')
    if not strace: raise ProbeFailure('spawn', 'strace_missing', details)
    args = [strace, '-f', '-qq', '-yy', '-s', '256', '-e',
            'trace=%file,write,writev,pwrite64,pwritev,pwritev2,ftruncate,mmap', '-o', str(trace), *map(str, command)]
    child = run_step('spawn', lambda: subprocess.Popen(args, env=env, cwd=cwd, stdin=subprocess.PIPE,
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                             start_new_session=True, preexec_fn=child_limits), details)
    try:
        stdout, stderr = run_step('wait', lambda: child.communicate(stdin, timeout=25), details)
    except subprocess.TimeoutExpired:
        try: os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError: pass
        try: run_step('reap_after_timeout', lambda: child.communicate(timeout=5), details)
        except subprocess.TimeoutExpired:
            raise ProbeFailure('reap_after_timeout', 'reap_timeout', details) from None
        details.update(exit_code=child.returncode, elapsed_ms=int((time.monotonic() - started) * 1000))
        raise ProbeFailure('wait', 'wall_timeout', details) from None
    details.update(exit_code=child.returncode, marker=stdout == MARKER,
                   stdout_bytes=len(stdout), stderr_bytes=len(stderr),
                   elapsed_ms=int((time.monotonic() - started) * 1000))
    if len(stdout) > 8192 or len(stderr) > 16384: raise ProbeFailure('output', 'native_output_bound', details)
    size = run_step('trace_stat', lambda: trace.stat().st_size, details)
    details['trace_bytes'] = size
    if size > TRACE_LIMIT: raise ProbeFailure('trace_stat', 'trace_bound', details)
    raw = run_step('trace_read', lambda: trace.read_text(errors='replace'), details)
    details['trace_lines'] = raw.count('\n')
    return child.returncode, stdout, len(stderr), raw


def files_under(root, owned=None, cache=None):
    """Bounded fd-relative lstat inventory: never open or follow a symlink.
    Target classification is lexical metadata, not target existence/resolution.
    """
    owned = owned or root
    cache = cache or root
    found = {}
    links = dict(count=0, absolute_targets=0, relative_targets=0,
                 inside_case_lexical=0, outside_case_lexical=0,
                 inside_owned_lexical=0, outside_owned_lexical=0,
                 in_requested_cache=0, numeric_targets=0,
                 single_component_targets=0, readlink_failures=0)
    visited = 0

    def walk(directory, logical, depth):
        nonlocal visited
        if depth > 64: raise ValueError('cache_file_bound')
        with os.scandir(directory) as entries:
            for entry in entries:
                visited += 1
                if visited > 4096: raise ValueError('cache_file_bound')
                path = logical / entry.name
                info = os.stat(entry.name, dir_fd=directory, follow_symlinks=False)
                if stat.S_ISLNK(info.st_mode):
                    links['count'] += 1
                    if contained(path, cache): links['in_requested_cache'] += 1
                    try: target = os.readlink(entry.name, dir_fd=directory)
                    except OSError:
                        links['readlink_failures'] += 1
                        continue
                    absolute = os.path.isabs(target)
                    links['absolute_targets' if absolute else 'relative_targets'] += 1
                    if target.isdecimal(): links['numeric_targets'] += 1
                    if '/' not in target: links['single_component_targets'] += 1
                    lexical = Path(os.path.normpath(target if absolute else os.path.join(logical, target)))
                    links['inside_case_lexical' if contained(lexical, root) else 'outside_case_lexical'] += 1
                    links['inside_owned_lexical' if contained(lexical, owned) else 'outside_owned_lexical'] += 1
                elif stat.S_ISDIR(info.st_mode):
                    child = os.open(entry.name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=directory)
                    try: walk(child, path, depth + 1)
                    finally: os.close(child)
                elif stat.S_ISREG(info.st_mode):
                    found[str(path)] = info.st_size
    fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try: walk(fd, root, 0)
    finally: os.close(fd)
    return found, links


def environment(case):
    result = {'LANG': 'C.UTF-8', 'TZ': 'UTC', 'DENO_NO_UPDATE_CHECK': '1',
              'DENO_NO_PROMPT': '1', 'DO_NOT_TRACK': '1', 'PYTHONDONTWRITEBYTECODE': '1'}
    for key, name in [('HOME', 'home'), ('XDG_CACHE_HOME', 'xdg-cache'),
                      ('XDG_CONFIG_HOME', 'xdg-config'), ('XDG_DATA_HOME', 'xdg-data'),
                      ('XDG_STATE_HOME', 'xdg-state'), ('XDG_RUNTIME_DIR', 'xdg-runtime'),
                      ('XDG_DATA_DIRS', 'xdg-data-dirs'), ('XDG_CONFIG_DIRS', 'xdg-config-dirs'),
                      ('TMPDIR', 'tmp'), ('TMP', 'tmp'), ('TEMP', 'tmp')]:
        path = case / name; path.mkdir(mode=0o700, exist_ok=True); result[key] = str(path)
    return result


def canary(base):
    case = base / 'canary'; case.mkdir(mode=0o700)
    cache = case / 'cache'; cache.mkdir(mode=0o700)
    outside = base / 'canary-outside-boundary'  # Still physically owned/private.
    inside = cache / 'inside'
    code = b'import os\nfor k in ("CANARY_INSIDE","CANARY_OUTSIDE"):\n f=open(os.environ[k],"wb");f.write(b"canary");f.close()\n'
    env = environment(case); env.update(CANARY_INSIDE=str(inside), CANARY_OUTSIDE=str(outside))
    trace = case / 'private.trace'
    status, _, _, raw = traced(['/usr/bin/python3', '-I', '-B', '-'], env, case, trace, code)
    receipt = scan_trace(raw, case, case, cache)
    trace.unlink()
    okay = (status == 0 and inside.read_bytes() == b'canary' and outside.read_bytes() == b'canary'
            and receipt['outside_successful_writes'] >= 1 and receipt['cache_write_attempts'] >= 1)
    if not okay: raise ValueError('positive_scanner_canary_failed')
    return dict(ok=True, outside_boundary_write_detected=True, inside_cache_write_detected=True)


def probe_case(base, name, deno):
    case = base / name; case.mkdir(mode=0o700)
    env = environment(case)
    cache = case / 'cache'
    if name == 'regular_file': cache.write_bytes(b'not a directory\n'); cache.chmod(0o600)
    else: cache.mkdir(mode=0o500 if name == 'readonly' else 0o700)
    env['DENO_DIR'] = str(cache)
    permissions_confirmed = True
    if name == 'readonly':
        try:
            with (cache / 'permission-canary').open('xb'): pass
        except PermissionError: pass
        else: permissions_confirmed = False
    if name == 'regular_file': permissions_confirmed = cache.is_file() and not cache.is_dir()
    if not permissions_confirmed: raise ValueError('case_permission_precondition_failed')
    attempts = []
    try:
        for attempt in range(2):
            diagnostic = dict(attempt=attempt + 1, completed_attempts=len(attempts))
            before, before_links = run_step('snapshot_before', lambda: files_under(case, base, cache), diagnostic)
            trace = case / 'private.trace'
            code, output, stderr_bytes, raw = traced([deno, *FLAGS], env, case, trace, SCRIPT, diagnostic)
            receipt = run_step('trace_parse_case', lambda: scan_trace(raw, case, case, cache, before), diagnostic)
            owned_receipt = run_step('trace_parse_owned', lambda: scan_trace(raw, base, case, cache, before), diagnostic)
            run_step('trace_unlink', trace.unlink, diagnostic)
            after, after_links = run_step('snapshot_after', lambda: files_under(case, base, cache), diagnostic)
            # Safe counts only. No filenames, cache contents, raw trace/errors.
            attempts.append(dict(exit_code=code, marker=output == MARKER, stderr_bytes=stderr_bytes,
                                 symlinks_before=before_links, symlinks_after=after_links,
                                 symlink_targets_classified_lexically_only=True,
                                 trace_bytes=diagnostic['trace_bytes'], trace_lines=diagnostic['trace_lines'],
                                 elapsed_ms=diagnostic['elapsed_ms'],
                                 cache_files=sum(contained(path, cache) for path in after),
                                 total_case_files=len(after), total_case_bytes=sum(after.values()),
                                 reused_case_files=len(before.keys() & after.keys()),
                                 outside_owned_write_attempts=owned_receipt['outside_write_attempts'],
                                 outside_owned_successful_writes=owned_receipt['outside_successful_writes'],
                                 **receipt))
    finally:
        if cache.is_dir(): cache.chmod(0o700)
    return dict(case=name, precondition_confirmed=permissions_confirmed, attempts=attempts)


def verify_component(directory, pins_path):
    pins = json.loads(pins_path.read_text())['platforms']['linux-x64']['deno']
    metadata = json.loads((directory / 'components.json').read_text())['components']['deno']
    path = directory / metadata['filename']
    if (pins['version'] != '2.9.8' or metadata['version'] != pins['version']
            or metadata['asset_sha256'] != pins['sha256'] or metadata['upstream_url'] != pins['url']
            or metadata['filename'] != 'deno' or path.is_symlink() or not path.is_file() or digest(path) != metadata['sha256']):
        raise ValueError('pinned_component_verification_failed')
    return path.resolve()


def self_test():
    allowed = Path('/owned/case'); cache = allowed / 'cache'
    operands = '\n'.join([
        '11 symlink("/outside/source-data", "/owned/case/cache/link") = 0',
        '11 symlinkat("/outside/source-data", 5</owned/case/cache>, "link2") = 0',
        '11 symlink("/owned/case/data", "/outside/link") = 0',
        '11 renameat(5</owned/case/cache>, "old", 6</outside>, "new") = 0',
        '11 newfstatat(AT_FDCWD, "/outside/read-only",  <unfinished ...>',
        '12 openat(AT_FDCWD, "/owned/case/cache/db", O_RDWR|O_CREAT, 0600 <unfinished ...>',
        '11 <... newfstatat resumed>{st_mode=S_IFREG|0600}, 0) = 0',
        '12 <... openat resumed>) = 3</owned/case/cache/db>',
        '13 readlink("/outside/read-only",  <unfinished ...>',
        '14 unlink("/outside/mutation" <unfinished ...>',
    ])
    parsed = scan_trace(operands, allowed, allowed, cache)
    assert parsed['outside_successful_writes'] == 2  # actual linkpath and rename destination
    assert parsed['cache_write_attempts'] == 4
    assert parsed['reassembled_operations'] == 2
    assert parsed['unresolved_read_operations'] == 1 and parsed['unresolved_mutating_operations'] == 1
    assert parsed['unresolved_syscalls'] == {'readlink': 1, 'unlink': 1}
    assert '/outside' not in json.dumps(parsed)
    read_only = scan_trace('15 newfstatat(AT_FDCWD, "file", <unfinished ...>', allowed, allowed, cache)
    assert read_only['unresolved_mutating_operations'] == 0 and read_only['unresolved_read_operations'] == 1
    with tempfile.TemporaryDirectory(prefix='nb-inventory-test-') as temporary:
        base = Path(temporary)
        case = base / 'case'; case.mkdir()
        cache = case / 'cache'; cache.mkdir()
        (cache / 'regular').write_bytes(b'static')
        elsewhere = base / 'elsewhere'; elsewhere.mkdir()
        (elsewhere / 'not_walked').write_bytes(b'outside_case')
        (cache / 'inside').symlink_to(cache / 'regular')
        (cache / 'outside').symlink_to('/outside/nonexistent-must-not-be-opened')
        (cache / 'directory').symlink_to('../../elsewhere', target_is_directory=True)
        (cache / 'numeric').symlink_to('12345')
        files, links = files_under(case, base, cache)
        assert files == {str(cache / 'regular'): 6}
        assert links['count'] == 4 and links['in_requested_cache'] == 4
        assert links['outside_owned_lexical'] == 1 and links['outside_case_lexical'] == 2
        assert links['numeric_targets'] == 1 and links['readlink_failures'] == 0
        assert str(cache) not in json.dumps(links)
    sensitive = 'PRIVATE https://secret.invalid/token /private/path'
    for exception, reason, errno in ((ValueError(sensitive), 'unclassified_value_error', None),
                                      (PermissionError(13, sensitive, sensitive), 'os_error', 13),
                                      (ValueError('cache_symlink_not_followed'), 'cache_symlink_not_followed', None)):
        def fail(): raise exception
        try: run_step('snapshot_after', fail, {'exit_code': 0, 'marker': True, 'trace_bytes': 123, 'raw': sensitive})
        except ProbeFailure as failure:
            public = failure.public()
            assert public['reason'] == reason and public['errno'] == errno
            assert public['exit_code'] == 0 and public['marker'] is True
            assert sensitive not in json.dumps(public) and 'raw' not in public
        else: raise AssertionError('diagnostic_failure_not_wrapped')
    allowed = Path('/owned/case'); cache = allowed / 'cache'
    lines = '\n'.join([
        '10 openat(AT_FDCWD, "/owned/case/cache/db", O_RDWR|O_CREAT, 0600) = 3</owned/case/cache/db>',
        '10 write(3</owned/case/cache/db>, "data", 4) = 4',
        '10 openat(AT_FDCWD, "/outside/db", O_RDWR|O_CREAT, 0600) = -1 EACCES (Permission denied)',
        '10 openat(AT_FDCWD, "/owned/case/cache/old", O_RDONLY) = 4</owned/case/cache/old>',
        '10 write(1<pipe:[123]>, "marker", 6) = 6',
        '10 mkdir("/outside/directory", 0700) = 0',
        '10 openat(AT_FDCWD, "relative", O_WRONLY|O_CREAT, 0600) = 5</owned/case/relative>',
    ])
    result = scan_trace(lines, allowed, allowed, cache, [cache / 'old'])
    assert result['outside_write_attempts'] == 2 and result['outside_successful_writes'] == 1
    assert result['cache_read_hits'] == 1 and result['cache_write_attempts'] == 2
    assert result['eacces_events'] == 1 and result['unresolved_file_operations'] == 0
    escaped = scan_trace('10 openat(AT_FDCWD, "/owned/case/../escape", O_WRONLY) = 3</owned/escape>', allowed, allowed, cache)
    assert escaped['outside_successful_writes'] == 1
    symlink = scan_trace('10 openat(AT_FDCWD, "/owned/case/link", O_RDWR) = 3</outside/target>', allowed, allowed, cache)
    assert symlink['outside_successful_writes'] == 1
    assert scan_trace('10 openat(AT_FDCWD, "x", O_RDWR <unfinished ...>', allowed, allowed, cache)['unresolved_file_operations'] == 1


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--components', type=Path, default=Path('desktop/src-tauri/binaries/youtube-light'))
    parser.add_argument('--pins', type=Path, default=Path('docs/YOUTUBE-COMPONENTS.json'))
    parser.add_argument('--out', type=Path, default=Path('.dev-check/deno-cache-probe/result.json'))
    parser.add_argument('--self-test', action='store_true')
    args = parser.parse_args()
    if args.self_test: self_test(); return
    if os.environ.get('GITHUB_ACTIONS') != 'true' or sys.platform != 'linux' or os.geteuid() == 0:
        raise SystemExit('gha_unprivileged_linux_only')
    report = dict(schema=1, version='2.9.8', sandbox_proven=False,
                  semantic_cache_hit_proven=False, cache_read_hit_scope_requested_deno_dir=True,
                  flags_match_pinned_non_npm_solver=True)
    code = 0
    stage = 'component_verification'
    try:
        deno = run_step('component_verification', lambda: verify_component(args.components, args.pins))
        with tempfile.TemporaryDirectory(prefix='nb-deno-cache-') as temporary:
            base = Path(temporary); base.chmod(0o700)
            stage = 'scanner_canary'
            report['scanner_canary'] = run_step('scanner_canary', lambda: canary(base))
            report['cases'] = []
            for name in ('writable', 'readonly', 'regular_file'):
                stage = name
                report['cases'].append(run_step('case_setup_or_cleanup', lambda: probe_case(base, name, deno)))
        report['observation_complete'] = True
        # Only positive-control execution is mandatory for this diagnostic;
        # fallback cases can honestly fail or write elsewhere, which is evidence.
        positive = report['cases'][0]['attempts']
        report['positive_execution_passed'] = all(a['exit_code'] == 0 and a['marker'] for a in positive)
        observations = [attempt for case in report['cases'] for attempt in case['attempts']]
        report['file_observation_passed'] = all(
            attempt['outside_owned_successful_writes'] == 0
            and attempt['unresolved_mutating_operations'] == 0 and attempt['unresolved_state_operations'] == 0
            and attempt['symlinks_after']['readlink_failures'] == 0
            for attempt in observations)
        if not report['positive_execution_passed'] or not report['file_observation_passed']: code = 1
    except ProbeFailure as failure:
        report.update(observation_complete=False, probe_error=True, error_stage=stage,
                      failure=failure.public())
        code = 2
    except (ValueError, KeyError, OSError, subprocess.SubprocessError):
        report.update(observation_complete=False, probe_error=True, error_stage=stage)
        code = 2
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(dict(observation_complete=report['observation_complete'], sandbox_proven=False)))
    raise SystemExit(code)

if __name__ == '__main__': main()
