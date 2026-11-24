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


def scan_trace(data, allowed, cwd, cache_root, existed=()):
    """Conservative file syscall receipt. Unknown forms are counted, never clean."""
    stats = dict(write_attempts=0, successful_writes=0, denied_writes=0,
                 outside_write_attempts=0, outside_successful_writes=0,
                 cache_write_attempts=0, cache_read_syscalls=0, cache_read_hits=0,
                 unresolved_file_operations=0, enotdir_events=0, eacces_events=0)
    known = {str(p) for p in existed}
    for line in data.splitlines():
        if 'ENOTDIR' in line: stats['enotdir_events'] += 1
        if 'EACCES' in line: stats['eacces_events'] += 1
        match = CALL.match(line)
        if not match:
            # strace -f may split one syscall into unfinished/resumed records.
            # Keep these visible rather than silently assigning a relative path.
            if '<unfinished ...>' in line or 'resumed>' in line:
                stats['unresolved_file_operations'] += 1
            continue
        call, arguments, result = match.groups()
        success = not result.startswith('-1')
        raw_paths = []
        if call.startswith('open') or call in WRITE_CALLS or call == 'chdir':
            raw_paths = [decoded_string(m.group()) for m in QUOTED.finditer(arguments)]
            if call.startswith('open'):
                raw_paths = raw_paths[:1]
                if success: raw_paths += ANNOTATED.findall(result)
        elif call in FD_WRITE_CALLS:
            raw_paths = ANNOTATED.findall(arguments[:arguments.find(',') if ',' in arguments else len(arguments)])
        elif call == 'mmap' and 'PROT_WRITE' in arguments and 'MAP_SHARED' in arguments:
            raw_paths = ANNOTATED.findall(arguments)
        else:
            continue
        paths = []
        for raw in raw_paths:
            if raw is None:
                stats['unresolved_file_operations'] += 1; continue
            if raw.startswith(('pipe:', 'socket:', 'anon_inode:', '/memfd:', '/anon_inode:')): continue
            raw = raw.removesuffix(' (deleted)')
            if not raw.startswith('/'):
                # Resolve AT_FDCWD against launch cwd; an annotated directory fd
                # supplies its own base. Deno's static script never calls chdir.
                annotations = ANNOTATED.findall(arguments.split(',', 1)[0])
                base = annotations[0] if annotations and annotations[0].startswith('/') else str(cwd)
                raw = os.path.normpath(os.path.join(base, raw))
            paths.append(Path(os.path.normpath(raw)))
        if call == 'chdir':
            if success: stats['unresolved_file_operations'] += 1
            continue
        writing = call in WRITE_CALLS or call in FD_WRITE_CALLS or call == 'mmap' or (
            call.startswith('open') and any(flag in arguments for flag in ('O_WRONLY', 'O_RDWR', 'O_CREAT', 'O_TRUNC')))
        if not paths and writing and not any(kind in arguments for kind in ('pipe:', 'socket:', 'anon_inode:', '/memfd:', '/anon_inode:')):
            stats['unresolved_file_operations'] += 1
        for path in set(paths):
            if writing:
                stats['write_attempts'] += 1
                stats['successful_writes' if success else 'denied_writes'] += 1
                if not contained(path, allowed):
                    stats['outside_write_attempts'] += 1
                    if success: stats['outside_successful_writes'] += 1
                if contained(path, cache_root): stats['cache_write_attempts'] += 1
            if call.startswith('open') and 'O_WRONLY' not in arguments and success and contained(path, cache_root):
                stats['cache_read_syscalls'] += 1
                if str(path) in known: stats['cache_read_hits'] += 1
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


def files_under(root):
    if not root.is_dir(): return {}
    found = {}
    for n, path in enumerate(root.rglob('*')):
        if n > 4096: raise ValueError('cache_file_bound')
        if path.is_symlink(): raise ValueError('cache_symlink_not_followed')
        if path.is_file(): found[str(path)] = path.stat().st_size
    return found


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
            before = run_step('snapshot_before', lambda: files_under(case), diagnostic)
            trace = case / 'private.trace'
            code, output, stderr_bytes, raw = traced([deno, *FLAGS], env, case, trace, SCRIPT, diagnostic)
            receipt = run_step('trace_parse_case', lambda: scan_trace(raw, case, case, cache, before), diagnostic)
            owned_receipt = run_step('trace_parse_owned', lambda: scan_trace(raw, base, case, cache, before), diagnostic)
            run_step('trace_unlink', trace.unlink, diagnostic)
            after = run_step('snapshot_after', lambda: files_under(case), diagnostic)
            # Safe counts only. No filenames, cache contents, raw trace/errors.
            attempts.append(dict(exit_code=code, marker=output == MARKER, stderr_bytes=stderr_bytes,
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
        if not report['positive_execution_passed']: code = 1
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
