#!/usr/bin/env python3
"""Pure tests for native_release_evidence.verified_native_jobs (API mocked)."""
import json
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from native_release_evidence import (verified_native_jobs, EvidenceRejected,
                                     JOB_NAMES)

REPO = 'owner/repo'
BUILD = '45971903126'
SOURCE = 'b' * 40
CURRENT_RUN = '45999999999'
EVIDENCE = {'windows': '45992046638', 'linux': '45990676700'}
HEADS = {'windows': 'd' * 40, 'linux': 'e' * 40}
CURRENT_HEAD = 'c' * 40

RECIPE = 'jobs:\n  runtime:\n    steps: [win]\n  linux-runtime:\n    steps: [lin]\n  other:\n    x: 1\n'
STEPS = [{'name': 'Verify native page save and offline MHTML reopen',
          'status': 'completed', 'conclusion': 'success'}]
TS = '2026-11-27T11:00:00.1234567Z '
WIN_LOG = TS + 'BUILD_RUN: ' + BUILD + '\n' + TS + 'other env\n'
LINUX_LOG = TS + 'BUILD_RUN: ' + BUILD + '\n' + TS + 'REQUEST: build\n'


def make_invoke(*, producer_head=SOURCE, evidence_meta=None, jobs=None,
                logs=None, desktop_diff='', changed='', recipes=None,
                current_overrides=None, fetches=None):
    evidence_meta = evidence_meta or {}
    logs = logs or {'windows': WIN_LOG, 'linux': LINUX_LOG}
    jobs = jobs if jobs is not None else {
        p: [{'id': 1000 + i, 'name': JOB_NAMES[p], 'status': 'completed',
             'conclusion': 'success', 'steps': STEPS}]
        for i, p in enumerate(('windows', 'linux'))}
    recipes = recipes or {}
    current = {'path': '.github/workflows/build.yml',
               'event': 'workflow_dispatch', 'head_sha': CURRENT_HEAD,
               'repository': {'full_name': REPO}}
    current.update(current_overrides or {})
    fetch_log = fetches if fetches is not None else []

    def meta(head, extra):
        value = {'status': 'completed', 'event': 'workflow_dispatch',
                 'path': '.github/workflows/build.yml', 'head_sha': head,
                 'repository': {'full_name': REPO}}
        value.update(extra)
        return value

    def invoke(command):
        if command[:2] == ['gh', 'api']:
            path = command[2].replace(f'repos/{REPO}/', '')
            if path == f'actions/runs/{CURRENT_RUN}':
                return json.dumps(current)
            if path == f'actions/runs/{BUILD}':
                return json.dumps(meta(producer_head, {}))
            for platform, run_id in EVIDENCE.items():
                if path == f'actions/runs/{run_id}':
                    return json.dumps(meta(HEADS[platform],
                                           evidence_meta.get(platform, {})))
                if path.startswith(f'actions/runs/{run_id}/jobs'):
                    return json.dumps({'total_count': len(jobs[platform]),
                                       'jobs': jobs[platform]})
            if path.startswith('actions/jobs/'):
                job_id = int(path.split('/')[2])
                for platform, entries in jobs.items():
                    if any(e['id'] == job_id for e in entries):
                        return logs[platform]
                raise AssertionError('unknown job id')
            raise AssertionError('unexpected api ' + path)
        if command[:2] == ['git', 'rev-parse']:
            return CURRENT_HEAD + '\n'
        if command[:2] == ['git', 'fetch']:
            fetch_log.append(command[4])
            return ''
        if command[:2] == ['git', 'show']:
            ref = command[2]
            if ref.startswith(CURRENT_HEAD):
                return recipes.get('current', RECIPE)
            for platform, head in HEADS.items():
                if ref.startswith(head):
                    return recipes.get(platform, RECIPE)
            raise AssertionError('unexpected show ' + ref)
        if command[:2] == ['git', 'diff']:
            spec = command[3]
            if spec.startswith(SOURCE + '..'):
                return desktop_diff
            if spec == f'{HEADS["windows"]}..{CURRENT_HEAD}':
                return changed.get('windows', '') if isinstance(changed, dict) else changed
            if spec == f'{HEADS["linux"]}..{CURRENT_HEAD}':
                return changed.get('linux', '') if isinstance(changed, dict) else changed
            raise AssertionError('unexpected diff ' + spec)
        raise AssertionError('unexpected command ' + command)
    return invoke


def run(request=None, **kwargs):
    return verified_native_jobs(REPO, BUILD, SOURCE, CURRENT_RUN,
                                request or dict(EVIDENCE),
                                invoke=make_invoke(**kwargs))


class NativeEvidenceTest(unittest.TestCase):
    def test_happy_path_returns_normalized_jobs_and_fetches(self):
        fetches = []
        result = run(fetches=fetches)
        self.assertEqual([j['name'] for j in result],
                         [JOB_NAMES['windows'], JOB_NAMES['linux']])
        for job, platform in zip(result, ('windows', 'linux')):
            self.assertEqual((job['status'], job['conclusion']),
                             ('completed', 'success'))
            self.assertEqual(job['steps'], STEPS)
            self.assertEqual(job['evidence_run'], EVIDENCE[platform])
            self.assertEqual(job['evidence_head'], HEADS[platform])
        self.assertEqual(fetches, [SOURCE, HEADS['windows'], HEADS['linux']])

    def test_same_combined_run_for_both_platforms_is_allowed(self):
        combined = {'windows': EVIDENCE['windows'], 'linux': EVIDENCE['windows']}
        both = [{'id': 1, 'name': JOB_NAMES['windows'], 'status': 'completed',
                 'conclusion': 'success', 'steps': STEPS},
                {'id': 2, 'name': JOB_NAMES['linux'], 'status': 'completed',
                 'conclusion': 'success', 'steps': STEPS}]
        logs = {'windows': WIN_LOG + TS + 'REQUEST: build\n',
                'linux': WIN_LOG + TS + 'REQUEST: build\n'}
        result = verified_native_jobs(
            REPO, BUILD, SOURCE, CURRENT_RUN, combined,
            invoke=make_invoke(jobs={'windows': both, 'linux': both},
                               logs=logs))
        self.assertEqual(len(result), 2)

    def test_overall_run_failure_with_successful_target_job_is_allowed(self):
        result = run(evidence_meta={p: {'conclusion': 'failure'}
                                    for p in ('windows', 'linux')})
        self.assertEqual(len(result), 2)

    def test_linux_may_change_windows_physical_but_windows_may_not(self):
        result = run(changed={'windows': '',
                              'linux': 'tests/gui/windows_physical.py\n'})
        self.assertEqual(len(result), 2)
        with self.assertRaisesRegex(EvidenceRejected,
                                    'unexpected_change_since_evidence:windows'):
            run(changed={'windows': 'tests/gui/windows_physical.py\n',
                         'linux': ''})

    def test_publication_only_and_docs_changes_are_allowed(self):
        run(changed='tests/prepare_release_draft.py\n'
                    'docs/verification/0.2.27/acceptance-state.json\n'
                    '.github/workflows/build.yml\n')

    def test_negatives(self):
        both_jobs = lambda **kw: {p: [dict({'id': i, 'name': JOB_NAMES[p],
            'status': 'completed', 'conclusion': 'success',
            'steps': STEPS}, **kw)] for i, p in enumerate(('windows', 'linux'))}
        cases = [
            ('producer_mismatch', dict(producer_head='f' * 40)),
            ('evidence_producer_binding', dict(logs={'windows': TS + 'BUILD_RUN: 111\n' + TS + 'BUILD_RUN: ' + BUILD + '\n', 'linux': LINUX_LOG})),
            ('evidence_producer_binding', dict(logs={'windows': TS + 'no marker\n', 'linux': LINUX_LOG})),
            ('evidence_producer_binding', dict(logs={'windows': 'x BUILD_RUN: ' + BUILD + '\n', 'linux': LINUX_LOG})),
            ('evidence_linux_request_not_full', dict(logs={'windows': WIN_LOG, 'linux': TS + 'BUILD_RUN: ' + BUILD + '\n' + TS + 'REQUEST: {"core_only":true}\n'})),
            ('desktop_changed_before_evidence', dict(desktop_diff='desktop/src-tauri/src/lib.rs\n')),
            ('unexpected_change_since_evidence', dict(changed='desktop/src/App.tsx\n')),
            ('unexpected_change_since_evidence', dict(changed='tests/gui/tab_routes.py\n')),
            ('native_recipe_changed', dict(recipes={'windows': RECIPE.replace('[win]', '[win2]')})),
            ('evidence_job_not_success', dict(jobs={p: [{'id': 1, 'name': JOB_NAMES[p],
                'status': 'completed', 'conclusion': 'failure', 'steps': STEPS}]
                for p in ('windows', 'linux')})),
            ('evidence_job_ambiguous', dict(jobs={p: [
                {'id': 1, 'name': JOB_NAMES[p], 'status': 'completed',
                 'conclusion': 'success', 'steps': STEPS},
                {'id': 2, 'name': JOB_NAMES[p], 'status': 'completed',
                 'conclusion': 'success', 'steps': STEPS}] for p in ('windows', 'linux')})),
            ('evidence_steps_missing', dict(jobs=both_jobs(steps=[]))),
            ('current_run_invalid', dict(current_overrides={'event': 'push'})),
            ('current_run_invalid', dict(current_overrides={'head_sha': 'f' * 40})),
            ('current_run_invalid', dict(current_overrides={'repository': {'full_name': 'other/repo'}})),
            ('producer_mismatch', dict()),
        ]
        # repository mismatch для producer/evidence подаётся отдельно
        for expected, kwargs in cases[:-1]:
            with self.subTest(expected=expected):
                with self.assertRaisesRegex(EvidenceRejected, expected):
                    run(**kwargs)

    def test_foreign_repository_meta_rejected(self):
        def foreign(command, _base=make_invoke()):
            if command[:2] == ['gh', 'api'] and command[2].endswith(f'actions/runs/{BUILD}'):
                return json.dumps({'status': 'completed', 'head_sha': SOURCE,
                                   'path': '.github/workflows/build.yml',
                                   'repository': {'full_name': 'evil/repo'}})
            return _base(command)
        with self.assertRaisesRegex(EvidenceRejected, 'producer_mismatch'):
            verified_native_jobs(REPO, BUILD, SOURCE, CURRENT_RUN, dict(EVIDENCE),
                                 invoke=foreign)

    def test_request_validation(self):
        for bad in (None, [], {'windows': '1'},
                    {'windows': '1', 'linux': '2', 'extra': '3'},
                    {'windows': 'abc', 'linux': '2'},
                    {'windows': '1' * 21, 'linux': '2'},
                    {'windows': 5, 'linux': '6'}):
            with self.subTest(bad=bad), self.assertRaises(EvidenceRejected):
                verified_native_jobs(REPO, BUILD, SOURCE, CURRENT_RUN, bad,
                                     invoke=make_invoke())

    def test_evidence_run_must_be_dispatch_of_build_yml(self):
        with self.assertRaisesRegex(EvidenceRejected, 'evidence_run_invalid:windows'):
            run(evidence_meta={'windows': {'event': 'push'}})
        with self.assertRaisesRegex(EvidenceRejected, 'evidence_run_invalid:windows'):
            run(evidence_meta={'windows': {'path': '.github/workflows/other.yml'}})

    def test_workflow_predicates_skip_normal_build_on_evidence_dispatch(self):
        import yaml
        workflow = yaml.safe_load((Path(__file__).resolve().parents[1]
                                   / '.github' / 'workflows' / 'build.yml').read_text())
        jobs = workflow['jobs']
        self.assertIn('publish-from-evidence', jobs)
        skip_markers = ("inputs.publish_existing_build == ''",
                        "inputs.runtime_build != ''",
                        "inputs.linux_runtime_build != ''",
                        "inputs.performance_build != ''",
                        "inputs.youtube_native_reuse != ''",
                        "inputs.youtube_muxer_reuse != ''",
                        "inputs.frontend_only",
                        "inputs.youtube_deno_cache_probe",
                        "inputs.youtube_consumer_package",
                        "inputs.youtube_probe_only", "refs/tags")
        for name, job in jobs.items():
            condition = ' '.join(str(job.get('if', '')).split())
            if name == 'publish-from-evidence':
                self.assertIn("inputs.publish_existing_build != ''", condition)
                continue
            self.assertTrue(any(m in condition for m in skip_markers),
                            (name, condition[:90]))
        for key in ('runtime', 'linux-runtime'):
            condition = ' '.join(str(jobs[key].get('if', '')).split())
            self.assertNotIn('publish_existing_build', condition)


def main():
    suite = unittest.TestLoader().loadTestsFromTestCase(NativeEvidenceTest)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() else 1


if __name__ == '__main__':
    sys.exit(main())
