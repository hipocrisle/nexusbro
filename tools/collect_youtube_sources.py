#!/usr/bin/env python3
"""Bounded CI source/notices collector. Inventory is never legal clearance.
No upstream code, DEPS, build scripts or extracted executables are executed.
"""
import argparse
import ast
import base64
import posixpath
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


def has_grant(data):
    # Actual rights/conditions prose, not an SPDX label or README reference.
    text = b' '.join(data.lower().split())
    if len(text) < 64: return False
    permissive = any(marker in text for marker in (
        b'permission is hereby granted', b'redistribution and use in source and binary forms',
        b'this is free and unencumbered software', b'permission to use, copy, modify, and'))
    apache = b'apache license' in text and b'terms and conditions for use, reproduction, and distribution' in text
    gnu = b'general public license' in text and b'you can redistribute it and/or modify' in text
    mpl = b'mozilla public license' in text and b'this source code form is subject to' in text
    cc0 = b'creative commons' in text and b'waiver' in text and b'copyright and related rights' in text
    return permissive or apache or gnu or mpl or cc0


def crate_material(data, name, version):
    result, auxiliary = [], {}
    with tarfile.open(fileobj=io.BytesIO(data), mode='r:gz') as archive:
        members = archive.getmembers()
        if len(members) > 20000 or sum(m.size for m in members) > 256 * 1024 ** 2:
            raise ValueError('crate_expanded_bound')
        explicit_license = None
        try:
            import tomllib
            manifest = archive.extractfile(f'{name}-{version}/Cargo.toml').read()
            package = tomllib.loads(manifest.decode()).get('package', {})
            candidate = package.get('license-file')
            if isinstance(candidate, str):
                explicit_license = safe_path(candidate).as_posix()
        except (ImportError, KeyError, ValueError, AttributeError):
            pass
        for member in members:
            rel = safe_path(member.name)
            if rel.parts[0] != f'{name}-{version}': raise ValueError('crate_root_mismatch')
            if not member.isdir() and not member.isfile():
                if is_notice(member.name): raise ValueError('notice_not_regular')
                continue
            leaf = rel.name
            if member.isfile() and (is_notice(member.name) or leaf in ('Cargo.toml', '.cargo_vcs_info.json')
                                    or leaf.upper().startswith('README')
                                    or '/'.join(rel.parts[1:]) == explicit_license):
                if member.size > 2 * 1024 ** 2: raise ValueError('notice_file_bound')
                contents = archive.extractfile(member).read()
                if is_notice(member.name): result.append((rel.as_posix(), contents))
                auxiliary['/'.join(rel.parts[1:])] = contents
        return result, auxiliary


def crate_notices(data, name, version):
    return crate_material(data, name, version)[0]


def github_repository(value):
    if not isinstance(value, str): return None
    if value.startswith('git+'): value = value[4:]
    parsed = urllib.parse.urlsplit(value)
    parts = parsed.path.strip('/').split('/')
    if parsed.scheme != 'https' or parsed.hostname != 'github.com' or len(parts) != 2:
        return None
    repo = '/'.join(parts).removesuffix('.git')
    return repo if re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', repo) else None


def repository_relative(directory, path):
    if not isinstance(path, str) or '\\' in path or path.startswith('/') or '\0' in path:
        raise ValueError('unsafe_repository_path')
    joined = posixpath.normpath(posixpath.join(directory, path))
    safe_path(joined)
    return joined


def resolve_deps(data):
    """Non-executing bounded DEPS subset. Conditions are data, never evaluated."""
    if len(data) > 128 * 1024: raise ValueError('deps_source_bound')
    module = ast.parse(data)
    if sum(1 for _ in ast.walk(module)) > 8192: raise ValueError('deps_ast_bound')
    assignments = {}
    for node in module.body:
        if isinstance(node, ast.Assign):
            for target in node.targets:
                if isinstance(target, ast.Name) and target.id in ('vars', 'deps'):
                    if target.id in assignments: raise ValueError('duplicate_deps_assignment')
                    assignments[target.id] = node.value
    variables = {}

    def value(node, depth=0):
        if depth > 32: raise ValueError('deps_ast_depth')
        if isinstance(node, ast.Constant) and isinstance(node.value, (str, bool, int, type(None))):
            if isinstance(node.value, str) and len(node.value) > 4096: raise ValueError('deps_string_bound')
            return node.value
        if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Add):
            left, right = value(node.left, depth + 1), value(node.right, depth + 1)
            if not isinstance(left, str) or not isinstance(right, str) or len(left) + len(right) > 4096:
                raise ValueError('deps_concat_type_or_bound')
            return left + right
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Name) and len(node.args) == 1 and not node.keywords:
            arg = node.args[0]
            if not isinstance(arg, ast.Constant) or not isinstance(arg.value, str): raise ValueError('deps_call_argument')
            if node.func.id == 'Str': return arg.value
            if node.func.id == 'Var' and arg.value in variables: return variables[arg.value]
            raise ValueError('deps_unknown_call')
        if isinstance(node, (ast.List, ast.Tuple)):
            return [value(item, depth + 1) for item in node.elts]
        if isinstance(node, ast.Dict):
            result = {}
            for key, item in zip(node.keys, node.values):
                if not isinstance(key, ast.Constant) or not isinstance(key.value, str): raise ValueError('deps_dict_key')
                if key.value in result: raise ValueError('deps_duplicate_dict_key')
                result[key.value] = value(item, depth + 1)
            return result
        raise ValueError('deps_unsupported_ast')

    raw_vars = assignments.get('vars')
    if not isinstance(raw_vars, ast.Dict): raise ValueError('deps_missing_vars')
    for key, item in zip(raw_vars.keys, raw_vars.values):
        if not isinstance(key, ast.Constant) or not isinstance(key.value, str): raise ValueError('deps_var_key')
        try: variables[key.value] = value(item)
        except ValueError: pass  # A dependent Var remains explicitly unresolved.
    raw_deps = assignments.get('deps')
    if not isinstance(raw_deps, ast.Dict): raise ValueError('deps_missing_dict')
    records = []
    for key, item in zip(raw_deps.keys, raw_deps.values):
        if not isinstance(key, ast.Constant) or not isinstance(key.value, str): raise ValueError('deps_path_key')
        path = key.value; safe_path(path)
        try:
            parsed = value(item)
            spec = parsed if isinstance(parsed, dict) else dict(url=parsed)
            url = spec.get('url')
            condition = spec.get('condition')
            record = dict(path=path, condition=condition if isinstance(condition, str) else None)
            if spec.get('dep_type') == 'cipd': record.update(status='cipd_not_fetched')
            elif isinstance(url, str) and '@' in url:
                origin, revision = url.rsplit('@', 1)
                endpoint = urllib.parse.urlsplit(origin)
                if not re.fullmatch('[0-9a-f]{40}', revision): record.update(status='nonexact_revision_not_fetched')
                elif endpoint.scheme != 'https' or endpoint.hostname not in GITILES_HOSTS or endpoint.username or endpoint.password or endpoint.port or endpoint.query or endpoint.fragment:
                    record.update(status='nonallowlisted_origin_not_fetched')
                else: record.update(status='resolved_exact_public', origin=origin.rstrip('/'), commit=revision)
            else: record.update(status='unsupported_dependency_not_fetched')
        except (ValueError, TypeError):
            record = dict(path=path, condition=None, status='unsupported_ast_not_fetched')
        record['notice_scope'] = ('optional_build_or_test_superset' if path.startswith(('test/', 'tools/', 'build', 'agents/'))
                                  else 'unknown_native_linkage')
        records.append(record)
    if len(records) > 256: raise ValueError('deps_entry_bound')
    return records


def decode_gitiles_json(data):
    if not data.startswith(b")]}'\n"): raise ValueError('gitiles_json_prefix')
    return json.loads(data[5:])


GITILES_HOSTS = ('chromium.googlesource.com', 'v8.googlesource.com',
                 'skia.googlesource.com', 'boringssl.googlesource.com')


class PrimaryRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, msg, headers, newurl):
        old, new = urllib.parse.urlsplit(request.full_url), urllib.parse.urlsplit(newurl)
        if new.scheme != 'https' or new.hostname not in (
                'raw.githubusercontent.com', 'api.github.com', 'static.crates.io', 'codeload.github.com', *GITILES_HOSTS):
            raise ValueError('source_redirect_host')
        if request.has_header('Authorization') and new.hostname != old.hostname:
            raise ValueError('authenticated_api_crosshost_redirect')
        return super().redirect_request(request, fp, code, msg, headers, newurl)


class Collector:
    def __init__(self, out):
        self.out = out
        self.deadline = time.monotonic() + 17 * 60
        self.lock = threading.Lock()
        self.sources = self.notices = 0
        self.records, self.gaps = [], []
        self.repository_archives, self.workspace_packages = [], []
        self.tree_cache = {}
        self.tree_lock = threading.Lock()
        self.native_dependency_notices = []
        self.gitiles_notice_count = 0
        self.required_native_notice_scopes = set()
        self.optional_native_notice_scopes = set()
        self.native_notice_priority = []
        out.mkdir(parents=True, exist_ok=False)

    def gap(self, scope, reason, category=None):
        if category is None:
            category = ('unknown_linkage_notice_coverage' if 'grant' in reason or 'notice' in reason
                        else 'source_or_binary_provenance_review')
        with self.lock:
            self.gaps.append(dict(scope=scope, reason=reason, category=category,
                                  required_distribution_notice_missing=category == 'required_distribution_notice_missing'))

    def download(self, url, limit=DOWNLOAD_LIMIT):
        if time.monotonic() >= self.deadline: raise ValueError('collector_deadline')
        parsed = urllib.parse.urlsplit(url)
        if parsed.scheme != 'https' or parsed.hostname not in (
            'raw.githubusercontent.com', 'api.github.com', 'static.crates.io', 'codeload.github.com', *GITILES_HOSTS):
            raise ValueError('nonprimary_source_host')
        headers = {'User-Agent': 'NexusBro-source-inventory/2'}
        if parsed.hostname == 'api.github.com':
            token = os.environ.get('GITHUB_TOKEN', '')
            if token and len(token) <= 8192:
                headers['Authorization'] = 'Bearer ' + token
        request = urllib.request.Request(url, headers=headers)
        # Ephemeral API-only token above; no cookies, ambient proxy or upstream execution.
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), PrimaryRedirect())
        with opener.open(request, timeout=20) as response:
            final = urllib.parse.urlsplit(response.geturl())
            if final.scheme != 'https' or final.hostname not in ('raw.githubusercontent.com', 'api.github.com', 'static.crates.io', 'codeload.github.com', *GITILES_HOSTS):
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
            with self.lock:
                self.workspace_packages.append(dict(name=name, version=version,
                    coverage='exact_Deno_source_archive_see_repository_archives_review_status'))
            return
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
            notices, auxiliary = crate_material(data, name, version)
            for filename, contents in notices:
                self.store('notices/crates/' + filename, contents, url, 'inside_verified_crate', True)
            grant = any(len(PurePosixPath(filename).parts) == 2 and has_grant(contents)
                        for filename, contents in notices)
            if not grant:
                grant = self.recover_crate_notice(ident, url, auxiliary)
            if not grant: self.gap(ident, 'actual_license_grant_not_proven_in_source_or_exact_vcs')
        except (ValueError, OSError, urllib.error.URLError, tarfile.TarError):
            self.gap(ident, 'source_or_notice_collection_failed')

    def repository_tree(self, repo, commit):
        key = (repo, commit)
        # Serial cache fill avoids duplicate huge API fetches for sibling crates.
        with self.tree_lock:
            if key in self.tree_cache: return self.tree_cache[key]
            if len(self.tree_cache) >= 128: raise ValueError('repository_tree_cache_bound')
            url = f'https://api.github.com/repos/{repo}/git/trees/{commit}?recursive=1'
            data = self.download(url, INDEX_LIMIT)
            tree = json.loads(data)
            if tree.get('truncated'): raise ValueError('repository_tree_truncated')
            entries = {x['path']: x for x in tree.get('tree', []) if x.get('type') == 'blob'}
            if len(entries) > 100000: raise ValueError('repository_tree_entry_bound')
            self.tree_cache[key] = entries
            return entries

    def recover_crate_notice(self, ident, crate_url, auxiliary):
        try:
            import tomllib
            package = tomllib.loads(auxiliary.get('Cargo.toml', b'').decode()).get('package', {})
            license_file = package.get('license-file')
            # A nonempty explicit in-archive file may carry a grant under an
            # unconventional name. README must contain actual grant prose.
            for filename, contents in auxiliary.items():
                if (filename == license_file or ('/' not in filename and filename.upper().startswith('README'))) and has_grant(contents):
                    self.store('notices/recovered/' + ident + '/' + filename, contents,
                               crate_url, 'grant_in_verified_crate_file', True)
                    return True
            vcs = json.loads(auxiliary.get('.cargo_vcs_info.json', b'{}'))
            if vcs.get('git', {}).get('dirty') is True: return False
            commit = vcs.get('git', {}).get('sha1', '')
            directory = vcs.get('path_in_vcs', '').strip('/')
            if directory: safe_path(directory)
            repo = github_repository(package.get('repository'))
            if not repo or not re.fullmatch('[0-9a-f]{40}', commit): return False
            entries = self.repository_tree(repo, commit)
            parents = ['']
            if directory:
                parts = directory.split('/')
                parents = ['/'.join(parts[:i]) for i in range(len(parts), -1, -1)]
            candidates = []
            if isinstance(license_file, str):
                candidates.append(repository_relative(directory, license_file))
            for path in sorted(entries):
                parent = str(PurePosixPath(path).parent)
                if parent == '.': parent = ''
                if parent in parents and (is_notice(path) or PurePosixPath(path).name.upper().startswith('README')):
                    candidates.append(path)
            found = False
            for path in dict.fromkeys(candidates):
                entry = entries.get(path)
                if not entry or entry.get('size', 0) > 2 * 1024 ** 2 or entry.get('mode') != '100644': continue
                safe_path(path)
                url = f'https://raw.githubusercontent.com/{repo}/{commit}/{urllib.parse.quote(path)}'
                contents = self.download(url, 2 * 1024 ** 2)
                git_sha = hashlib.sha1(b'blob ' + str(len(contents)).encode() + b'\0' + contents).hexdigest()
                if git_sha != entry['sha']: raise ValueError('recovered_notice_git_blob_mismatch')
                # Empty/test fixture files do not satisfy grant coverage.
                if not has_grant(contents): continue
                self.store('notices/recovered/' + ident + '/' + path, contents, url,
                           'crate_vcs_exact_commit_and_upstream_tree_git_blob_sha1', True)
                found = True
            return found
        except (ValueError, KeyError, OSError, urllib.error.URLError):
            return False

    def source_archive(self, pin):
        repo, commit = pin['repository'], pin['commit']
        expected = pin.get('archive_sha256', '')
        if not re.fullmatch('[0-9a-f]{40}', commit) or (expected and not re.fullmatch('[0-9a-f]{64}', expected)):
            raise ValueError('invalid_repository_pin')
        url = f'https://codeload.github.com/{repo}/tar.gz/{commit}'
        try:
            data = self.download(url)
            actual = sha(data)  # BEFORE parsing any tar headers.
            if expected and actual != expected: raise ValueError('source_archive_sha256_mismatch')
            prefix = repo.split('/')[1] + '-' + commit
            evidence = {
                'denoland/deno': {'Cargo.toml', 'cli/Cargo.toml', 'libs/deno_v8/Cargo.toml', 'libs/core/Cargo.toml', '.github/workflows/ci.generated.yml'},
                'denoland/rusty_v8': {'Cargo.toml', 'build.rs', '.gitmodules', '.github/workflows/ci.yml', '.github/workflows/release.yml'},
                'denoland/v8': {'DEPS', 'BUILD.gn'},
            }.get(repo, set())
            expanded, count = 0, 0
            with tarfile.open(fileobj=io.BytesIO(data), mode='r|gz') as archive:
                for member in archive:
                    if time.monotonic() >= self.deadline: raise ValueError('collector_deadline')
                    rel = safe_path(member.name)
                    if rel.parts[0] != prefix: raise ValueError('repository_archive_root_mismatch')
                    count += 1; expanded += member.size
                    if count > 100000 or expanded > 512 * 1024 ** 2: raise ValueError('repository_archive_expanded_bound')
                    if not (member.isdir() or member.isfile() or member.issym()):
                        raise ValueError('repository_archive_special_entry')
                    relative = '/'.join(rel.parts[1:])
                    if member.isfile() and relative in evidence:
                        if member.size > 2 * 1024 * 1024: raise ValueError('native_source_evidence_bound')
                        contents = archive.extractfile(member).read()
                        self.store('provenance/native-source/' + repo.replace('/', '-') + '/' + relative,
                                   contents, url, 'reviewed_archive_sha256' if expected else 'unreviewed_archive_source_evidence')
                        if repo == 'denoland/v8' and relative == 'DEPS':
                            try: self.v8_deps_notices(contents, self.required_native_notice_scopes)
                            except (ValueError, KeyError):
                                self.gap('V8', 'safe_deps_resolution_incomplete', 'unknown_native_linkage_notice_coverage')
                    if member.issym():
                        target = repository_relative('/'.join(rel.parts[:-1]), member.linkname)
                        if PurePosixPath(target).parts[0] != prefix: raise ValueError('repository_link_escape')
            self.store('sources/repos/' + repo.replace('/', '-') + '-' + commit + '.tar.gz',
                       data, url, 'reviewed_archive_sha256' if expected else 'bootstrap_sha256_unreviewed_exact_commit_url')
            self.repository_archives.append(dict(repository=repo, commit=commit, sha256=actual,
                                                  reviewed=bool(expected), bytes=len(data), complete_source_archive=True))
            if not expected: self.gap(repo, 'source_archive_bootstrap_sha256_requires_review')
        except (ValueError, OSError, urllib.error.URLError, tarfile.TarError):
            self.gap(repo, 'source_archive_collection_or_verification_failed')
            self.repository_archives.append(dict(repository=repo, commit=commit, reviewed=False, complete_source_archive=False))

    def gitiles_notices(self, dependency, required=False):
        origin, commit = dependency['origin'], dependency['commit']
        prefix = hashlib.sha256((origin + '@' + commit).encode()).hexdigest()[:16]
        queue = [('', 0)]
        visited = 0
        found, grants = 0, 0
        while queue:
            directory, depth = queue.pop(0)
            visited += 1
            if visited > 8: raise ValueError('gitiles_directory_bound')
            location = urllib.parse.quote(directory, safe='/')
            tree_url = f'{origin}/+/{commit}/{location}?format=JSON'
            tree = decode_gitiles_json(self.download(tree_url, 1024 * 1024))
            entries = tree.get('entries', [])
            if len(entries) > 2048: raise ValueError('gitiles_tree_entry_bound')
            for entry in entries:
                name = entry.get('name', '')
                safe_path(name)
                if '/' in name: raise ValueError('gitiles_entry_name')
                path = '/'.join(x for x in (directory, name) if x)
                kind = entry.get('type')
                if kind == 'tree' and depth < 2 and name.lower() in ('licenses', 'license', 'copying', 'icu4c'):
                    queue.append((path, depth + 1))
                elif kind == 'blob' and (is_notice(name) or name.upper().startswith('README') or name == 'PATENTS'):
                    if self.gitiles_notice_count >= 128: raise ValueError('gitiles_notice_total_bound')
                    if not re.fullmatch('[0-9a-f]{40}', entry.get('id', '')): raise ValueError('gitiles_blob_id')
                    text_url = f'{origin}/+/{commit}/{urllib.parse.quote(path, safe="/")}?format=TEXT'
                    encoded = self.download(text_url, 2 * 1024 * 1024)
                    contents = base64.b64decode(b''.join(encoded.split()), validate=True)
                    if len(contents) > 1536 * 1024: raise ValueError('gitiles_decoded_notice_bound')
                    git_sha = hashlib.sha1(b'blob ' + str(len(contents)).encode() + b'\0' + contents).hexdigest()
                    if git_sha != entry['id']: raise ValueError('gitiles_blob_mismatch')
                    self.store('notices/native-deps/' + prefix + '/' + path, contents, text_url,
                               'exact_commit_gitiles_tree_blob_sha1_and_sha256', True)
                    self.gitiles_notice_count += 1
                    found += 1
                    if has_grant(contents): grants += 1
        dependency.update(notice_files=found, grant_text_files=grants,
                          required_distribution_notice_missing=required and grants == 0,
                          notice_status='grant_prose_collected' if grants else 'grant_prose_unresolved')
        if not grants:
            self.gap(dependency['path'], 'exact_native_dependency_grant_prose_not_collected',
                     'required_distribution_notice_missing' if required else 'unknown_native_linkage_notice_coverage')

    def v8_deps_notices(self, data, required_scopes):
        dependencies = resolve_deps(data)
        for dependency in dependencies:
            if dependency['path'] in self.optional_native_notice_scopes:
                dependency['notice_scope'] = 'optional_build_or_test_superset'
        priority = {name: i for i, name in enumerate(self.native_notice_priority)}
        dependencies.sort(key=lambda item: (item['path'] not in required_scopes,
                                            priority.get(item['path'], 1000)))
        fetched = {}
        for dependency in dependencies:
            required = dependency['path'] in required_scopes
            dependency['reviewed_required_scope'] = required
            # Tests/build tools are inventory only, never a source/testdata fetch.
            if dependency['notice_scope'] == 'optional_build_or_test_superset' and not required:
                dependency['notice_status'] = 'optional_inventory_only'
            elif dependency['status'] != 'resolved_exact_public':
                dependency['notice_status'] = 'not_fetched_unresolved'
                self.gap(dependency['path'], dependency['status'],
                         'required_distribution_notice_missing' if required else 'unknown_native_linkage_notice_coverage')
            else:
                key = (dependency['origin'], dependency['commit'])
                if key in fetched:
                    previous = fetched[key]
                    for field in ('notice_files', 'grant_text_files', 'notice_status'):
                        if field in previous: dependency[field] = previous[field]
                    if required and not dependency.get('grant_text_files'):
                        self.gap(dependency['path'], 'shared_exact_dependency_grant_unresolved', 'required_distribution_notice_missing')
                elif len(fetched) >= 32:
                    dependency['notice_status'] = 'repository_bound_not_fetched'
                    self.gap(dependency['path'], 'gitiles_repository_total_bound',
                             'required_distribution_notice_missing' if required else 'unknown_native_linkage_notice_coverage')
                else:
                    fetched[key] = dependency
                    try: self.gitiles_notices(dependency, required)
                    except (ValueError, KeyError, OSError, urllib.error.URLError):
                        dependency['notice_status'] = 'exact_dependency_collection_incomplete'
                        self.gap(dependency['path'], 'gitiles_notice_collection_incomplete',
                                 'required_distribution_notice_missing' if required else 'unknown_native_linkage_notice_coverage')
        self.native_dependency_notices.extend(dependencies)
        self.gap('V8', 'conservative_notice_superset_not_exact_native_linked_graph', 'provenance_not_exact_linked_graph')

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
                    # DEPS coverage now lives in bounded AST/Gitiles inventory above.
        except (ValueError, KeyError, OSError, urllib.error.URLError):
            self.gap(repo, 'repository_provenance_collection_incomplete')

    def finish(self):
        inventory = dict(schema=1, product_notices_cleared=False,
                         dependency_scope='all_Deno_Cargo_lock_superset_not_exact_linked_binary_graph',
                         sources_bytes=self.sources, notices_bytes=self.notices,
                         repository_archives=self.repository_archives, workspace_packages=self.workspace_packages,
                         native_dependency_notices=self.native_dependency_notices,
                         files=sorted(self.records, key=lambda item: item['filename']),
                         pending_gaps=sorted(self.gaps, key=lambda item: (item['scope'], item['reason'])))
        data = (json.dumps(inventory, indent=2) + '\n').encode()
        if len(data) > INDEX_LIMIT: raise ValueError('index_bound')
        (self.out / 'index.json').write_bytes(data)
        (self.out / 'gaps.json').write_text(json.dumps(dict(product_notices_cleared=False, gaps=inventory['pending_gaps']), indent=2) + '\n')


def collect(pins, source_pins, out):
    import tomllib  # CI Python3.12; offline archive tests also support3.10.
    c = Collector(out)
    c.required_native_notice_scopes = set(source_pins.get('required_native_notice_scopes', []))
    c.optional_native_notice_scopes = set(source_pins.get('optional_build_test_notice_scopes', []))
    c.native_notice_priority = source_pins.get('native_notice_priority', [])
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
        for pin in source_pins['repositories']:
            c.source_archive(pin)
            c.repository_provenance(pin['repository'], pin['commit'], pin['commit'])
        deno = next((x for x in c.repository_archives if x['repository'] == 'denoland/deno'), None)
        if not deno or not deno.get('complete_source_archive'):
            c.gap('Deno-workspace', 'complete_workspace_source_archive_missing')
        elif not deno.get('reviewed'):
            c.gap('Deno-workspace', 'complete_workspace_archive_present_sha256_review_pending')
        with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
            list(pool.map(c.crate, packages))
        c.gap('yt-dlp-PyInstaller', 'aggregate_notice_retained_exact_bundled_dependency_source_versions_not_machine_proven')
        c.gap('deno-native-V8', 'source_build_recipe_recorded_binary_asset_linkage_not_asserted', 'native_asset_provenance_not_a_notice_failure')
        c.gap('distribution', 'source_offer_retention_and_license_review_not_completed')
    except (ValueError, KeyError, OSError, urllib.error.URLError):
        c.gap('collector', 'collection_incomplete_fail_closed')
        raise
    finally:
        c.finish()


def self_test():
    revision = 'a' * 40
    sample = ("vars = {'host': Str('https://chromium.googlesource.com'), 'rev': '" + revision + "'}\n"
              "deps = {'third_party/icu': {'url': Var('host') + '/chromium/deps/icu.git@' + Var('rev'), "
              "'condition': '__import__(do_not_execute)'}, 'test/data': 'https://chromium.googlesource.com/v8/data@HEAD', "
              "'third_party/bad': __import__('os').system('do_not_execute')}\n")
    resolved = resolve_deps(sample.encode())
    assert resolved[0]['status'] == 'resolved_exact_public'
    assert resolved[0]['commit'] == revision
    assert resolved[0]['condition'] == '__import__(do_not_execute)'
    assert resolved[1]['status'] == 'nonexact_revision_not_fetched'
    assert resolved[1]['notice_scope'] == 'optional_build_or_test_superset'
    assert resolved[2]['status'] == 'unsupported_ast_not_fetched'
    assert decode_gitiles_json(b")]}'\n{\"entries\": []}") == {'entries': []}
    try: decode_gitiles_json(b'{"entries": []}')
    except ValueError: pass
    else: raise AssertionError('gitiles_xssi_prefix_ignored')
    assert not has_grant(b'')
    assert not has_grant(b'Licensed under Apache License 2.0. See the license file for details.' * 2)
    assert has_grant(b'Permission is hereby granted, free of charge, to any person obtaining a copy of this software.')
    assert repository_relative('crates/member', '../../LICENSE') == 'LICENSE'
    for relative in ('../../../escape', '/absolute', 'bad\\name'):
        try: repository_relative('crates/member', relative)
        except ValueError: pass
        else: raise AssertionError('repository_escape_accepted')
    assert github_repository('https://github.com/owner/project.git') == 'owner/project'
    assert github_repository('https://evil.invalid/owner/project') is None
    request = urllib.request.Request('https://api.github.com/repos/example/tree', headers={'Authorization': 'Bearer FAKE'})
    try: PrimaryRedirect().redirect_request(request, None, 302, '', {}, 'https://raw.githubusercontent.com/example/path')
    except ValueError: pass
    else: raise AssertionError('api_token_crosshost_redirect_accepted')
    import tempfile
    grant = b'Permission is hereby granted, free of charge, to any person obtaining a copy of this software.'
    blob_id = hashlib.sha1(b'blob ' + str(len(grant)).encode() + b'\0' + grant).hexdigest()
    tree_bytes = b")]}'\n" + json.dumps({'entries': [{'name': 'LICENSE', 'type': 'blob', 'id': blob_id}]}).encode()
    native = dict(origin='https://chromium.googlesource.com/chromium/deps/icu.git', commit='a' * 40, path='third_party/icu')
    with tempfile.TemporaryDirectory() as temporary:
        good = Collector(Path(temporary) / 'good')
        good.download = lambda url, *_: tree_bytes if url.endswith('format=JSON') else base64.b64encode(grant)
        good.gitiles_notices(dict(native))
        assert good.gitiles_notice_count == 1 and not good.gaps
        bad = Collector(Path(temporary) / 'bad')
        bad.download = lambda url, *_: tree_bytes if url.endswith('format=JSON') else base64.b64encode(grant + b'tampered')
        try: bad.gitiles_notices(dict(native))
        except ValueError: pass
        else: raise AssertionError('gitiles_blob_tampering_accepted')
        assert not bad.records
        absent = Collector(Path(temporary) / 'absent')
        absent.download = lambda *_: b")]}'\n{\"entries\": []}"
        absent.gitiles_notices(dict(native), required=True)
        assert absent.gaps[0]['required_distribution_notice_missing'] is True
        optional = Collector(Path(temporary) / 'optional')
        optional.download = absent.download
        optional.gitiles_notices(dict(native))
        assert optional.gaps[0]['required_distribution_notice_missing'] is False
    commit = 'a' * 40
    archive_bytes = io.BytesIO()
    with tarfile.open(fileobj=archive_bytes, mode='w:gz') as archive:
        entry = tarfile.TarInfo('demo-' + commit + '/LICENSE')
        text = b'Permission is hereby granted, free of charge, to any person obtaining a copy of this software.'
        entry.size = len(text)
        archive.addfile(entry, io.BytesIO(text))
    with tempfile.TemporaryDirectory() as temp:
        collector = Collector(Path(temp) / 'valid')
        collector.download = lambda *_: archive_bytes.getvalue()
        collector.source_archive(dict(repository='owner/demo', commit=commit, archive_sha256=sha(archive_bytes.getvalue())))
        assert collector.repository_archives[0]['reviewed'] is True
        assert collector.repository_archives[0]['complete_source_archive'] is True
        assert not collector.gaps
        rejected = Collector(Path(temp) / 'invalid')
        rejected.download = lambda *_: archive_bytes.getvalue()
        rejected.source_archive(dict(repository='owner/demo', commit=commit, archive_sha256='0' * 64))
        assert rejected.repository_archives[0]['complete_source_archive'] is False
        assert not list((Path(temp) / 'invalid').rglob('*.tar.gz'))
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
    parser.add_argument('--source-pins', type=Path, default=Path('docs/YOUTUBE-SOURCE-PINS.json'))
    parser.add_argument('--out', type=Path, default=Path('.dev-check/youtube-source-inventory'))
    parser.add_argument('--self-test', action='store_true')
    args = parser.parse_args()
    if args.self_test: self_test(); return
    if os.environ.get('GITHUB_ACTIONS') != 'true': raise SystemExit('ci_only')
    collect(json.loads(args.pins.read_text()), json.loads(args.source_pins.read_text()), args.out)

if __name__ == '__main__': main()
