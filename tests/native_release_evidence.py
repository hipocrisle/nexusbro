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
BUILD_RUN_IN_LOG = re.compile(r'\bBUILD_RUN: ([1-9][0-9]{0,19})\b')


class EvidenceRejected(ValueError):
    pass


def _default_invoke(command):
    result = subprocess.run(command, capture_output=True, text=True, timeout=180)
    if result.returncode != 0:
        raise EvidenceRejected('evidence_command_failed:' + command[0])
    return result.stdout


def _api(invoke, repo, path):
    return json.loads(invoke(['gh', 'api', f'repos/{repo}/{path}']))


def _validate_request(request):
    if not isinstance(request, dict) or set(request) != {'windows', 'linux'}:
        raise EvidenceRejected('native_evidence_request_invalid')
    for value in request.values():
        if not isinstance(value, str) or not RUN_ID.fullmatch(value):
            raise EvidenceRejected('native_evidence_run_id_invalid')
    if request['windows'] == request['linux']:
        raise EvidenceRejected('native_evidence_runs_ambiguous')
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
    if not (isinstance(repo, str) and repo.count('/') == 1):
        raise EvidenceRejected('repo_invalid')
    if not (isinstance(build, str) and RUN_ID.fullmatch(build)):
        raise EvidenceRejected('build_invalid')
    if not (isinstance(source, str) and SHA.fullmatch(source)):
        raise EvidenceRejected('source_invalid')
    if not (isinstance(current_run, str) and RUN_ID.fullmatch(current_run)):
        raise EvidenceRejected('current_run_invalid')
    request = _validate_request(request)
    current_meta = _api(invoke, repo, f'actions/runs/{current_run}')
    if not str(current_meta.get('path', '')).endswith('build.yml'):
        raise EvidenceRejected('current_run_invalid')
    producer = _api(invoke, repo, f'actions/runs/{build}')
    if producer.get('status') != 'completed' or producer.get('head_sha') != source:
        raise EvidenceRejected('producer_mismatch')
    current_head = invoke(['git', 'rev-parse', 'HEAD']).strip()
    if not SHA.fullmatch(current_head):
        raise EvidenceRejected('current_head_invalid')
    current_recipe_text = invoke(
        ['git', 'show', f'{current_head}:.github/workflows/build.yml'])
    normalized = []
    for platform in ('windows', 'linux'):
        run_id = request[platform]
        meta = _api(invoke, repo, f'actions/runs/{run_id}')
        if (meta.get('status') != 'completed'
                or meta.get('event') != 'workflow_dispatch'
                or not str(meta.get('path', '')).endswith('build.yml')):
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
        if set(BUILD_RUN_IN_LOG.findall(log)) != {build}:
            raise EvidenceRejected('evidence_producer_binding:' + platform)
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
