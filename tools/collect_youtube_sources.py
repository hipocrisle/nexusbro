#!/usr/bin/env python3
"""Bounded CI source/notices collector. Inventory is never legal clearance.
No upstream code, DEPS, build scripts or extracted executables are executed.
"""
import argparse
import concurrent.futures
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import tarfile
import threading
import time
import urllib.error
import urllib.parse
import urllib.request

SOURCE_LIMIT = 1024 ** 3
NOTICE_LIMIT = 16 * 1024 ** 2
INDEX_LIMIT = 8 * 1024 ** 2
DOWNLOAD_LIMIT = 64 * 1024 ** 2
LOCK_SHA = '36aab78913808a9fb2e5a3a3a27a74a1a1045011133f28beaac6eabf57fc9895'
V8_COMMIT = 'ac1e23989121713ca642f6650b34deff7b686896'


def sha(data): return hashlib.sha256(data).hexdigest()


def safe_path(name):
    if '\\' in name or '\x00' in name or name.startswith('/'):
        raise ValueError('unsafe_path')
    parts = name.split('/')
    if not parts or any(x in ('', '.', '..') for x in parts):
        raise ValueError('unsafe_path')
    return PurePosixPath(name)


def is_notice(name):
    base = PurePosixPath(name).name.upper()
    return base.startswith(('LICENSE', 'LICENCE', 'COPYING', 'NOTICE', 'AUTHORS', 'COPYRIGHT'))


def crate_notices(data, name, version):
    # Original checksum-verified source archive retained independently. Only
    # regular notice bytes are copied; no link, device or path is extracted.
    result = []
    with tarfile.open(fileobj=io.BytesIO(data), mode='r:gz') as archive:
        members = archive.getmembers()
        if len(members) > 20000 or sum(m.size for m in members) > 256 * 1024 ** 2:
            raise ValueError('crate_expanded_bound')
        for member in members:
            rel = safe_path(member.name)
            if rel.parts[0] != f'{name}-{version}': raise ValueError('crate_root_mismatch')
            if not member.isdir() and not member.isfile():
                # Preserve source tar but never follow archive links, even when
                # a project places its license behind one.
                if is_notice(member.name): raise ValueError('notice_not_regular')
                continue
            if member.isfile() and is_notice(member.name):
                if member.size > 2 * 1024 ** 2: raise ValueError('notice_file_bound')
                result.append((rel.as_posix(), archive.extractfile(member).read()))
    return result


class Collector:
    def __init__(self, out):
        self.out = out
        self.deadline = time.monotonic() + 17 * 60
        self.lock = threading.Lock()
        self.sources = self.notices = 0
        self.records, self.gaps = [], []
        out.mkdir(parents=True, exist_ok=False)

    def gap(self, scope, reason):
        with self.lock: self.gaps.append(dict(scope=scope, reason=reason))

    def download(self, url, limit=DOWNLOAD_LIMIT):
        if time.monotonic() >= self.deadline: raise ValueError('collector_deadline')
        parsed = urllib.parse.urlsplit(url)
        if parsed.scheme != 'https' or parsed.hostname not in (
            'raw.githubusercontent.com', 'api.github.com', 'static.crates.io'):
            raise ValueError('nonprimary_source_host')
        request = urllib.request.Request(url, headers={'User-Agent': 'NexusBro-source-inventory/1'})
        # No credentials, cookies, ambient proxy or upstream executable calls.
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        with opener.open(request, timeout=20) as response:
            final = urllib.parse.urlsplit(response.geturl())
            if final.scheme != 'https' or final.hostname not in ('raw.githubusercontent.com', 'api.github.com', 'static.crates.io'):
                raise ValueError('source_redirect_host')
            data = bytearray()
            while True:
                if time.monotonic() >= self.deadline: raise ValueError('collector_deadline')
                block = response.read(min(1024 * 1024, limit + 1 - len(data)))
                if not block: return bytes(data)
                data.extend(block)
                if len(data) > limit: raise ValueError('source_download_bound')

    def store(self, rel, data, url, verification, notice=False):
        safe_path(rel)
        with self.lock:
            if notice:
                if self.notices + len(data) > NOTICE_LIMIT: raise ValueError('notices_total_bound')
                self.notices += len(data)
            else:
                if self.sources + len(data) > SOURCE_LIMIT: raise ValueError('sources_total_bound')
                self.sources += len(data)
            path = self.out / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            with path.open('xb') as f: f.write(data)
            self.records.append(dict(filename=rel, bytes=len(data), sha256=sha(data),
                                     source_url=url, verification=verification))

    def pinned(self, pin, prefix, notice):
        data = self.download(pin['url'])
        if sha(data) != pin['sha256']: raise ValueError('pinned_sha256_mismatch')
        self.store(prefix + '/' + pin['filename'], data, pin['url'], 'pinned_sha256', notice)
        return data

    def crate(self, package):
        name, version = package['name'], package['version']
        ident = name + '-' + version
        if not re.fullmatch(r'[A-Za-z0-9_.+-]+', ident):
            self.gap('cargo', 'invalid_package_identifier'); return
        source = package.get('source', '')
        if not source:
            self.gap(ident, 'workspace_source_in_deno_snapshot_not_archived'); return
        if source != 'registry+https://github.com/rust-lang/crates.io-index':
            self.gap(ident, 'nonregistry_source_requires_exact_git_snapshot'); return
        checksum = package.get('checksum', '')
        if not re.fullmatch('[0-9a-f]{64}', checksum):
            self.gap(ident, 'missing_lock_checksum'); return
        url = f'https://static.crates.io/crates/{name}/{name}-{version}.crate'
        try:
            data = self.download(url)
            if sha(data) != checksum: raise ValueError('crate_sha256_mismatch')
            self.store('sources/crates/' + ident + '.crate', data, url, 'cargo_lock_sha256')
            notices = crate_notices(data, name, version)
            for filename, contents in notices:
                self.store('notices/crates/' + filename, contents, url, 'inside_verified_crate', True)
            if not notices: self.gap(ident, 'no_regular_notice_file_found_in_verified_source')
        except (ValueError, OSError, urllib.error.URLError, tarfile.TarError):
            self.gap(ident, 'source_or_notice_collection_failed')

    def repository_provenance(self, repo, revision, expected_commit=None):
        # GitHub API resolves exact tag to commit. Mutable tag identity remains
        # a gap; hash-verified individual Git blobs do NOT authenticate a tag.
        prefix = repo.replace('/', '-')
        try:
            commit_url = f'https://api.github.com/repos/{repo}/commits/{revision}'
            commit = json.loads(self.download(commit_url, 2 * 1024 ** 2))
            commit_id = commit['sha']
            if not re.fullmatch('[0-9a-f]{40}', commit_id): raise ValueError('invalid_commit')
            if expected_commit and commit_id != expected_commit: raise ValueError('commit_mismatch')
            if not expected_commit: self.gap(repo, 'tag_resolution_requires_independent_commit_pin_review')
            tree_url = f'https://api.github.com/repos/{repo}/git/trees/{commit["commit"]["tree"]["sha"]}?recursive=1'
            tree_data = self.download(tree_url, INDEX_LIMIT)
            tree = json.loads(tree_data)
            self.store('provenance/' + prefix + '-tree.json', tree_data, tree_url, 'upstream_git_tree_metadata')
            if tree.get('truncated'): self.gap(repo, 'upstream_git_tree_truncated')
            candidates = [entry for entry in tree.get('tree', []) if entry.get('type') == 'blob' and
                          (is_notice(entry['path']) or entry['path'] in ('DEPS', '.gitmodules', 'Cargo.lock'))]
            if len(candidates) > 512:
                self.gap(repo, 'repository_notice_candidate_bound'); candidates = candidates[:512]
            for entry in candidates:
                path = entry['path']; safe_path(path)
                if entry.get('size', 0) > 2 * 1024 ** 2:
                    self.gap(repo, 'repository_notice_file_bound'); continue
                url = f'https://raw.githubusercontent.com/{repo}/{commit_id}/{urllib.parse.quote(path)}'
                data = self.download(url, 2 * 1024 ** 2)
                git_sha = hashlib.sha1(b'blob ' + str(len(data)).encode() + b'\0' + data).hexdigest()
                if git_sha != entry['sha']: raise ValueError('git_blob_mismatch')
                self.store('notices/repos/' + prefix + '/' + path, data, url, 'upstream_tree_git_blob_sha1', True)
                if path == 'DEPS':
                    # Only literal exact revision references; never eval DEPS.
                    references = sorted(set(re.findall(rb'https://[^\s\"\']+@[0-9a-f]{40}', data)))
                    self.store('provenance/' + prefix + '-deps-literal-revisions.json',
                               (json.dumps([x.decode('ascii') for x in references], indent=2) + '\n').encode(),
                               url, 'literal_references_only_not_selected_native_graph')
                    self.gap(repo, 'deps_saved_unexecuted_conditions_cipd_and_native_build_selection_unresolved')
            self.gap(repo, 'full_corresponding_source_archive_hash_not_pinned_no_archive_downloaded')
        except (ValueError, KeyError, OSError, urllib.error.URLError):
            self.gap(repo, 'repository_provenance_collection_incomplete')

    def finish(self):
        inventory = dict(schema=1, product_notices_cleared=False,
                         dependency_scope='all_Deno_Cargo_lock_superset_not_exact_linked_binary_graph',
                         sources_bytes=self.sources, notices_bytes=self.notices,
                         files=sorted(self.records, key=lambda item: item['filename']),
                         pending_gaps=sorted(self.gaps, key=lambda item: (item['scope'], item['reason'])))
        data = (json.dumps(inventory, indent=2) + '\n').encode()
        if len(data) > INDEX_LIMIT: raise ValueError('index_bound')
        (self.out / 'index.json').write_bytes(data)
        (self.out / 'gaps.json').write_text(json.dumps(dict(product_notices_cleared=False, gaps=inventory['pending_gaps']), indent=2) + '\n')


def collect(pins, out):
    import tomllib  # CI Python3.12; offline archive tests also support3.10.
    c = Collector(out)
    try:
        for pin in pins['licenses']: c.pinned(pin, 'notices/pinned', True)
        lock = None
        for pin in pins['source_inventory']:
            data = c.pinned(pin, 'provenance/pinned', False)
            if pin['filename'] == 'DENO-Cargo.lock':
                if sha(data) != LOCK_SHA: raise ValueError('unsupported_deno_lock')
                lock = tomllib.loads(data.decode())
        if lock is None: raise ValueError('missing_deno_lock')
        packages = lock.get('package', [])
        if len(packages) > 2048: raise ValueError('cargo_package_bound')
        with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
            list(pool.map(c.crate, packages))
        for repo, rev, exact in [('denoland/deno', 'v2.9.8', None),
                                 ('denoland/rusty_v8', 'v150.4.0', None),
                                 ('denoland/v8', V8_COMMIT, V8_COMMIT),
                                 ('yt-dlp/yt-dlp', '2026.11.23', None)]:
            c.repository_provenance(repo, rev, exact)
        c.gap('yt-dlp-PyInstaller', 'aggregate_notice_retained_exact_bundled_dependency_source_versions_not_machine_proven')
        c.gap('deno-native-V8', 'prebuilt_v8_asset_provenance_selected_DEPS_and_release_feature_graph_not_proven')
        c.gap('distribution', 'source_offer_retention_and_license_review_not_completed')
    except (ValueError, KeyError, OSError, urllib.error.URLError):
        c.gap('collector', 'collection_incomplete_fail_closed')
        raise
    finally:
        c.finish()


def self_test():
    for bad in ('../escape', '/abs', 'a/../b', 'a\\b', 'a//b'):
        try: safe_path(bad)
        except ValueError: pass
        else: raise AssertionError('unsafe_path_accepted')
    payload = io.BytesIO()
    with tarfile.open(fileobj=payload, mode='w:gz') as archive:
        entry = tarfile.TarInfo('demo-1.0/LICENSE'); data = b'actual notice'; entry.size = len(data)
        archive.addfile(entry, io.BytesIO(data))
    assert crate_notices(payload.getvalue(), 'demo', '1.0') == [('demo-1.0/LICENSE', b'actual notice')]
    linked = io.BytesIO()
    with tarfile.open(fileobj=linked, mode='w:gz') as archive:
        entry = tarfile.TarInfo('demo-1.0/LICENSE')
        entry.type = tarfile.SYMTYPE
        entry.linkname = '/etc/private'
        archive.addfile(entry)
    try: crate_notices(linked.getvalue(), 'demo', '1.0')
    except ValueError: pass
    else: raise AssertionError('notice_symlink_accepted')
    try: crate_notices(payload.getvalue(), 'other', '1.0')
    except ValueError: pass
    else: raise AssertionError('wrong_crate_root_accepted')


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--pins', type=Path, default=Path('docs/YOUTUBE-COMPONENTS.json'))
    parser.add_argument('--out', type=Path, default=Path('.dev-check/youtube-source-inventory'))
    parser.add_argument('--self-test', action='store_true')
    args = parser.parse_args()
    if args.self_test: self_test(); return
    if os.environ.get('GITHUB_ACTIONS') != 'true': raise SystemExit('ci_only')
    collect(json.loads(args.pins.read_text()), args.out)

if __name__ == '__main__': main()
