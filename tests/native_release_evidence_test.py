#!/usr/bin/env python3
"""Pure tests for native_release_evidence.verified_native_jobs (API mocked)."""
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from native_release_evidence import (verified_native_jobs, EvidenceRejected,
                                     JOB_NAMES, RECIPE_KEYS)

REPO = 'o/r'
BUILD = '45971903126'
SOURCE = 'b' * 40
CURRENT_RUN = '45999999999'
EVIDENCE = {'windows': '45992046638', 'linux': '45990676700'}
HEADS = {'windows': 'd' * 40, 'linux': 'e' * 40}
CURRENT_HEAD = 'c' * 40

RECIPE = 'jobs:\n  runtime:\n    steps: [win]\n  linux-runtime:\n    steps: [lin]\n  other:\n    x: 1\n'
STEPS = [{'name': 'Verify native page save and offline MHTML reopen',
          'status': 'completed', 'conclusion': 'success'}]


def make_invoke(*, producer_head=SOURCE, evidence_meta=None, jobs=None,
                log='env\nBUILD_RUN: 45971903126\nmore', desktop_diff='',
                changed='', recipes=None, current_path='.github/workflows/build.yml'):
    evidence_meta = evidence_meta or {}
    jobs = jobs if jobs is not None else {
        p: [{'id': 1000 + i, 'name': JOB_NAMES[p], 'status': 'completed',
             'conclusion': 'success', 'steps': STEPS}] for i, p in enumerate(('windows', 'linux'))}
    recipes = recipes or {}

    def invoke(command):
        if command[:2] == ['gh', 'api']:
            path = command[2].replace(f'repos/{REPO}/', '')
            if path == f'actions/runs/{CURRENT_RUN}':
                return __import__('json').dumps({'path': current_path})
            if path == f'actions/runs/{BUILD}':
                return __import__('json').dumps(
                    {'status': 'completed', 'head_sha': producer_head})
            for platform, run_id in EVIDENCE.items():
                if path == f'actions/runs/{run_id}':
                    meta = {'status': 'completed', 'event': 'workflow_dispatch',
                            'path': '.github/workflows/build.yml',
                            'head_sha': HEADS[platform]}
                    meta.update(evidence_meta.get(platform, {}))
                    return __import__('json').dumps(meta)
                if path.startswith(f'actions/runs/{run_id}/jobs'):
                    return __import__('json').dumps(
                        {'total_count': len(jobs[platform]),
                         'jobs': jobs[platform]})
            if path.startswith('actions/jobs/'):
                return log
            raise AssertionError('unexpected api ' + path)
        if command[:2] == ['git', 'rev-parse']:
            return CURRENT_HEAD + '\n'
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
            if spec == f'{SOURCE}..{HEADS["windows"]}' or spec == f'{SOURCE}..{HEADS["linux"]}':
                return desktop_diff
            if spec == f'{HEADS["windows"]}..{CURRENT_HEAD}':
                return changed.get('windows', '') if isinstance(changed, dict) else changed
            if spec == f'{HEADS["linux"]}..{CURRENT_HEAD}':
                return changed.get('linux', '') if isinstance(changed, dict) else changed
            raise AssertionError('unexpected diff ' + spec)
        raise AssertionError('unexpected command ' + command)
    return invoke


def run(**kwargs):
    return verified_native_jobs(REPO, BUILD, SOURCE, CURRENT_RUN, dict(EVIDENCE),
                                invoke=make_invoke(**kwargs))


class NativeEvidenceTest(unittest.TestCase):
    def test_happy_path_returns_normalized_jobs_with_provenance(self):
        result = run()
        self.assertEqual([j['name'] for j in result],
                         [JOB_NAMES['windows'], JOB_NAMES['linux']])
        for job, platform in zip(result, ('windows', 'linux')):
            self.assertEqual(job['status'], 'completed')
            self.assertEqual(job['conclusion'], 'success')
            self.assertEqual(job['steps'], STEPS)
            self.assertEqual(job['evidence_run'], EVIDENCE[platform])
            self.assertEqual(job['evidence_head'], HEADS[platform])

    def test_overall_run_failure_with_successful_target_job_is_allowed(self):
        meta = {p: {'conclusion': 'failure'} for p in ('windows', 'linux')}
        result = run(evidence_meta=meta)
        self.assertEqual(len(result), 2)

    def test_linux_may_change_windows_physical_but_windows_may_not(self):
        result = run(changed={'windows': '', 'linux': 'tests/gui/windows_physical.py\n'})
        self.assertEqual(len(result), 2)
        with self.assertRaisesRegex(EvidenceRejected, 'unexpected_change_since_evidence:windows'):
            run(changed={'windows': 'tests/gui/windows_physical.py\n', 'linux': ''})

    def test_publication_only_and_docs_changes_are_allowed(self):
        run(changed='tests/prepare_release_draft.py\ndocs/verification/0.2.27/acceptance-state.json\n.github/workflows/build.yml\n')

    def test_negatives(self):
        import copy
        cases = [
            ('producer_mismatch', dict(producer_head='f' * 40)),
            ('evidence_producer_binding', dict(log='BUILD_RUN: 111\nBUILD_RUN: 45971903126')),
            ('evidence_producer_binding', dict(log='no marker here')),
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
            ('evidence_steps_missing', dict(jobs={p: [{'id': 1, 'name': JOB_NAMES[p],
                'status': 'completed', 'conclusion': 'success', 'steps': []}]
                for p in ('windows', 'linux')})),
        ]
        for expected, kwargs in cases:
            with self.subTest(expected=expected):
                with self.assertRaisesRegex(EvidenceRejected, expected):
                    run(**kwargs)

    def test_request_validation(self):
        for bad in (None, [], {'windows': '1'},
                    {'windows': '1', 'linux': '2', 'extra': '3'},
                    {'windows': 'abc', 'linux': '2'},
                    {'windows': '1' * 21, 'linux': '2'},
                    {'windows': '5', 'linux': '5'},
                    {'windows': 5, 'linux': '6'}):
            with self.subTest(bad=bad), self.assertRaises(EvidenceRejected):
                verified_native_jobs(REPO, BUILD, SOURCE, CURRENT_RUN, bad,
                                     invoke=make_invoke())

    def test_evidence_run_must_be_dispatch_of_build_yml(self):
        meta = {'windows': {'event': 'push'}}
        with self.assertRaisesRegex(EvidenceRejected, 'evidence_run_invalid:windows'):
            run(evidence_meta=meta)


def main():
    suite = unittest.TestLoader().loadTestsFromTestCase(NativeEvidenceTest)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() else 1


if __name__ == '__main__':
    sys.exit(main())
