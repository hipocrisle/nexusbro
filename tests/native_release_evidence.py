#!/usr/bin/env python3
"""Bounded reuse of ALREADY-verified native acceptance jobs for a
publisher-only run. Verified provenance, never invented success:

- evidence runs: same repo, workflow_dispatch of build.yml, completed
  (overall failure allowed — only the target JOB must be success);
- target job (runtime / Native Linux from existing matching binary) is
  exactly one, completed+success, with its real steps preserved;
- producer binding comes from the trusted job LOG (BUILD_RUN: <id> of the
  source verification step), never from branch names or hand summaries;
- desktop/ is byte-identical source -> evidence_head and evidence_head ->
  current HEAD; non-desktop changes since evidence are limited to
  publication-only files (plus tests/gui/windows_physical.py for the Linux
  evidence only);
- the native job recipe block in .github/workflows/build.yml is byte-equal
  between the evidence head and the current head.

The caller runs its own validate_native_jobs() over the returned jobs.
Only small metadata/job logs are fetched; packages are never downloaded
here (the signature publisher still downloads and verifies them in CI).
"""
import json
import os
import re
import subprocess

JOB_NAMES = {'windows': 'runtime',
             'linux': 'Native Linux from existing matching binary'}
RECIPE_KEYS = {'windows': 'runtime', 'linux': 'linux-runtime'}
PUBLICATION_ONLY = frozenset({
    'tests/prepare_release_draft.py',
    'tests/prepare_release_draft_test.py',
    'tests/publish_tested_release.py',
    'tests/native_release_evidence.py',
    'tests/native_release_evidence_test.py',
})
PUBLICATION_ONLY_PREFIXES = ('docs/verification/',)
LINUX_EXTRA_IGNORE = frozenset({'tests/gui/windows_physical.py'})
RUN_ID = re.compile(r'^[1-9][0-9]{0,19}$')
SHA = re.compile(r'^[0-9a-f]{40}$')
REPO_NAME = re.compile(r'^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$')
WORKFLOW_PATH = '.github/workflows/build.yml'
# Anchored whole log lines with an optional ISO timestamp prefix, exactly as
# the runner prints env lines; never an arbitrary substring match.
ENV_PREFIX = r'(?:\d{4}-\d{2}-\d{2}T[0-9:.]+Z[ \t]+)?[ \t]*'
BUILD_RUN_LINE = re.compile(
    r'(?m)^' + ENV_PREFIX + r'BUILD_RUN: ([1-9][0-9]{0,19})[ \t]*\r?$')
LINUX_REQUEST_LINE = re.compile(
    r'(?m)^' + ENV_PREFIX + r'REQUEST: ([^\r\n]+)[ \t]*\r?$')


class EvidenceRejected(ValueError):
    pass


def _default_invoke(command):
    result = subprocess.run(command, capture_output=True, text=True, timeout=180)
    if result.returncode != 0 and command[:2]==['gh','api']:
        match=re.fullmatch(r'repos/([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)/actions/jobs/([0-9]+)/logs',command[-1])
        if match:
            # gh run view can retrieve the run-log archive when the direct
            # per-job redirect endpoint is unavailable to the CI token.
            fallback=subprocess.run(['gh','run','view','--repo',match[1],'--job',match[2],'--log'],capture_output=True,text=True,timeout=180)
            if fallback.returncode==0:
                return '\n'.join(line.split('\t',2)[-1] for line in fallback.stdout.splitlines())
            result=fallback
    if result.returncode != 0:
        # Fixed facts only: gh stderr can contain signed redirect URLs.
        status=re.search(r'HTTP[ \t]+([0-9]{3})',result.stderr or '')
        reason='http_'+status.group(1) if status else 'unclassified'
        if 'Resource not accessible by integration' in (result.stderr or ''):reason+=':integration_scope'
        if 'Must have admin rights' in (result.stderr or ''):reason+=':admin_required'
        operation='job_logs' if command[-1].endswith('/logs') else command[0]
        detail=result.stderr or ''
        for key in ('GH_TOKEN','GITHUB_TOKEN'):
            secret=os.environ.get(key,'')
            if secret:detail=detail.replace(secret,'<credential>')
        detail=re.sub(r'https?://\S+','<url>',detail)
        detail=re.sub(r'\b(?:gh[pousr]_[A-Za-z0-9_]+|github_pat_[A-Za-z0-9_]+)','<credential>',detail)
        detail=' '.join(detail.split())[:240]
        raise EvidenceRejected('evidence_command_failed:'+operation+':'+reason+':'+detail)
    return result.stdout


def _api(invoke, repo, path):
    return json.loads(invoke(['gh', 'api', f'repos/{repo}/{path}']))


def _run_meta(invoke, repo, run_id, code):
    meta = _api(invoke, repo, f'actions/runs/{run_id}')
    if (not isinstance(meta, dict)
            or str(meta.get('path', '')) != WORKFLOW_PATH
            or meta.get('repository', {}).get('full_name') != repo):
        raise EvidenceRejected(code)
    return meta


def _fetch(invoke, sha):
    invoke(['git', 'fetch', '--no-tags', 'origin', sha])


def _validate_request(request):
    if not isinstance(request, dict) or set(request) != {'windows', 'linux'}:
        raise EvidenceRejected('native_evidence_request_invalid')
    for value in request.values():
        if not isinstance(value, str) or not RUN_ID.fullmatch(value):
            raise EvidenceRejected('native_evidence_run_id_invalid')
    # The SAME run may legitimately carry both acceptance jobs (a combined
    # run); job-name uniqueness below still forbids any ambiguity.
    return request


def _recipe_block(text, key):
    lines = text.splitlines()
    start = None
    for index, line in enumerate(lines):
        if line == f'  {key}:':
            start = index
            break
    if start is None:
        raise EvidenceRejected('recipe_missing:' + key)
    block = [lines[start]]
    for line in lines[start + 1:]:
        if re.match(r'^  [a-z0-9_-]+:', line):
            break
        block.append(line)
    return '\n'.join(block).rstrip()


def verified_native_jobs(repo, build, source, current_run, request, invoke=None):
    invoke = invoke or _default_invoke
    if not (isinstance(repo, str) and REPO_NAME.fullmatch(repo)):
        raise EvidenceRejected('repo_invalid')
    if not (isinstance(build, str) and RUN_ID.fullmatch(build)):
        raise EvidenceRejected('build_invalid')
    if not (isinstance(source, str) and SHA.fullmatch(source)):
        raise EvidenceRejected('source_invalid')
    if not (isinstance(current_run, str) and RUN_ID.fullmatch(current_run)):
        raise EvidenceRejected('current_run_invalid')
    request = _validate_request(request)
    current_head = invoke(['git', 'rev-parse', 'HEAD']).strip()
    if not SHA.fullmatch(current_head):
        raise EvidenceRejected('current_head_invalid')
    current_meta = _run_meta(invoke, repo, current_run, 'current_run_invalid')
    if (current_meta.get('event') != 'workflow_dispatch'
            or current_meta.get('head_sha') != current_head):
        raise EvidenceRejected('current_run_invalid')
    producer = _run_meta(invoke, repo, build, 'producer_mismatch')
    if producer.get('status') != 'completed' or producer.get('head_sha') != source:
        raise EvidenceRejected('producer_mismatch')
    _fetch(invoke, source)
    current_recipe_text = invoke(
        ['git', 'show', f'{current_head}:{WORKFLOW_PATH}'])
    normalized = []
    for platform in ('windows', 'linux'):
        run_id = request[platform]
        meta = _run_meta(invoke, repo, run_id,
                         'evidence_run_invalid:' + platform)
        if (meta.get('status') != 'completed'
                or meta.get('event') != 'workflow_dispatch'):
            raise EvidenceRejected('evidence_run_invalid:' + platform)
        evidence_head = meta.get('head_sha')
        if not (isinstance(evidence_head, str) and SHA.fullmatch(evidence_head)):
            raise EvidenceRejected('evidence_head_invalid')
        jobs = _api(invoke,
                    repo, f'actions/runs/{run_id}/jobs?filter=latest&per_page=100')
        if jobs.get('total_count', 101) > 100:
            raise EvidenceRejected('evidence_jobs_incomplete')
        name = JOB_NAMES[platform]
        matches = [j for j in jobs.get('jobs', []) if j.get('name') == name]
        if len(matches) != 1:
            raise EvidenceRejected('evidence_job_ambiguous:' + platform)
        job = matches[0]
        if job.get('status') != 'completed' or job.get('conclusion') != 'success':
            raise EvidenceRejected('evidence_job_not_success:' + platform)
        if not isinstance(job.get('steps'), list) or not job['steps']:
            raise EvidenceRejected('evidence_steps_missing:' + platform)
        log = invoke(
            ['gh', 'api', f'repos/{repo}/actions/jobs/{job["id"]}/logs'])
        if set(BUILD_RUN_LINE.findall(log)) != {build}:
            raise EvidenceRejected('evidence_producer_binding:' + platform)
        if platform == 'linux' and {value.strip() for value in LINUX_REQUEST_LINE.findall(log)} != {build}:
            # A diagnostic/partial reuse request (JSON REQUEST) can pass all
            # of its own steps while skipping routes; only the plain full
            # native request line is acceptable evidence.
            raise EvidenceRejected('evidence_linux_request_not_full')
        _fetch(invoke, evidence_head)
        if invoke(['git', 'diff', '--name-only',
                   f'{source}..{evidence_head}', '--', 'desktop/']).strip():
            raise EvidenceRejected('desktop_changed_before_evidence:' + platform)
        allowed = set(PUBLICATION_ONLY)
        if platform == 'linux':
            allowed |= LINUX_EXTRA_IGNORE
        changed = invoke(
            ['git', 'diff', '--name-only', f'{evidence_head}..{current_head}'])
        for path in (line.strip() for line in changed.splitlines()):
            if not path:
                continue
            if (path in allowed
                    or any(path.startswith(p) for p in PUBLICATION_ONLY_PREFIXES)
                    or path == '.github/workflows/build.yml'):
                continue
            raise EvidenceRejected(
                'unexpected_change_since_evidence:' + platform + ':' + path)
        evidence_recipe = _recipe_block(
            invoke(['git', 'show',
                    f'{evidence_head}:.github/workflows/build.yml']),
            RECIPE_KEYS[platform])
        if evidence_recipe != _recipe_block(current_recipe_text,
                                            RECIPE_KEYS[platform]):
            raise EvidenceRejected('native_recipe_changed:' + platform)
        normalized.append({'name': name, 'status': 'completed',
                           'conclusion': 'success', 'steps': job['steps'],
                           'evidence_run': run_id,
                           'evidence_head': evidence_head})
    return normalized
