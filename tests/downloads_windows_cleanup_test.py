#!/usr/bin/env python3
"""Pure regression tests for downloads_windows_native cleanup helpers.

Runs on Linux with stubbed process/window objects: no Win32, no COM, no real
processes are touched. Proves the exact semantics the Windows run needs:
already-exited own editor is success, live/foreign/ambiguous states are not,
foreign Explorer windows are never closed, and no blanket except→success.
"""
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[0] / 'gui'))
from downloads_windows_native import close_owned_editor, close_owned_explorer


class FakeNoSuch(Exception):
    pass


class Proc:
    def __init__(self, *, terminate_error=None, running_after=False, running_error=None):
        self.terminate_error = terminate_error
        self.running_after = running_after
        self.running_error = running_error
        self.terminated = False

    def terminate(self):
        self.terminated = True
        if self.terminate_error:
            raise self.terminate_error

    def wait(self, timeout=None):
        return 0

    def is_running(self):
        if self.running_error:
            raise self.running_error
        return self.running_after


class EditorCleanupTest(unittest.TestCase):
    def test_already_exited_own_process_is_success(self):
        components = {}
        proc = Proc(terminate_error=FakeNoSuch(), running_after=False)
        self.assertTrue(close_owned_editor(proc, components, no_such_process=FakeNoSuch))
        self.assertEqual(components, {})

    def test_live_process_terminated_is_success(self):
        components = {}
        proc = Proc(running_after=False)
        self.assertTrue(close_owned_editor(proc, components, no_such_process=FakeNoSuch))
        self.assertTrue(proc.terminated)

    def test_still_running_is_not_success(self):
        components = {}
        proc = Proc(running_after=True)
        self.assertFalse(close_owned_editor(proc, components, no_such_process=FakeNoSuch))

    def test_other_error_is_not_success_and_is_typed(self):
        components = {}
        proc = Proc(terminate_error=RuntimeError('denied'))
        self.assertFalse(close_owned_editor(proc, components, no_such_process=FakeNoSuch))
        self.assertEqual(components['editor_error_type'], 'RuntimeError')

    def test_vanished_but_reports_running_is_not_success(self):
        components = {}
        proc = Proc(terminate_error=FakeNoSuch(), running_after=True)
        self.assertFalse(close_owned_editor(proc, components, no_such_process=FakeNoSuch))


class ExplorerCleanupTest(unittest.TestCase):
    PROOF = {'hwnd': 4242, 'pid': 77, 'birth': 1000.5}

    def run_cleanup(self, *, alive, pid, birth=1000.5, close_error=None,
                    gone_after_close=True):
        components = {}
        state = {'alive': alive, 'closed': False, 'uia_called': 0}

        def is_window(hwnd):
            assert hwnd == self.PROOF['hwnd']
            return state['alive'] and not state['closed']

        def pid_of(hwnd):
            return pid

        def birth_of(p):
            assert p == self.PROOF['pid']
            return birth

        def uia_close(hwnd, p):
            state['uia_called'] += 1
            if close_error:
                raise close_error
            if gone_after_close:
                state['closed'] = True

        def wait(check):
            if not check():
                raise TimeoutError('window did not close')

        result = close_owned_explorer(self.PROOF, components, is_window=is_window,
                                      pid_of=pid_of, birth_of=birth_of,
                                      uia_close=uia_close, wait=wait)
        return result, components, state

    def test_dead_hwnd_is_success_without_com(self):
        result, components, state = self.run_cleanup(alive=False, pid=77)
        self.assertTrue(result)
        self.assertEqual(state['uia_called'], 0)
        self.assertEqual(components, {})

    def test_foreign_pid_on_reused_hwnd_is_untouched_success(self):
        result, components, state = self.run_cleanup(alive=True, pid=999)
        self.assertTrue(result)
        self.assertEqual(state['uia_called'], 0, 'foreign window must never be closed')

    def test_own_live_window_closed_and_gone(self):
        result, components, state = self.run_cleanup(alive=True, pid=77)
        self.assertTrue(result)
        self.assertEqual(state['uia_called'], 1)

    def test_com_failure_on_live_window_is_not_success(self):
        class COMError(Exception):
            pass
        result, components, state = self.run_cleanup(alive=True, pid=77,
                                                     close_error=COMError('detached'))
        self.assertFalse(result)
        self.assertEqual(components['folder_error_type'], 'COMError')

    def test_uia_detaches_after_actual_close_verified_by_os(self):
        state = {'alive': True}
        def close(hwnd, pid):
            state['alive'] = False
            raise RuntimeError('UIA detached after close')
        components = {}
        self.assertTrue(close_owned_explorer(self.PROOF, components,
            is_window=lambda hwnd: state['alive'], pid_of=lambda hwnd: 77,
            birth_of=lambda pid: 1000.5, uia_close=close, wait=lambda check: None))

    def test_window_disappears_between_iswindow_and_pid_lookup(self):
        state = {'alive': True}
        def pid_of(hwnd):
            state['alive'] = False
            raise RuntimeError('window disappeared')
        self.assertTrue(close_owned_explorer(self.PROOF, {},
            is_window=lambda hwnd: state['alive'], pid_of=pid_of,
            birth_of=lambda pid: 1000.5, uia_close=lambda hwnd, pid: self.fail('must not close'),
            wait=lambda check: None))

    def test_birth_mismatch_refuses_to_close(self):
        result, components, state = self.run_cleanup(alive=True, pid=77, birth=2000.0)
        self.assertFalse(result)
        self.assertEqual(state['uia_called'], 0)

    def test_window_surviving_close_is_not_success(self):
        result, components, state = self.run_cleanup(alive=True, pid=77,
                                                     gone_after_close=False)
        self.assertFalse(result)
        self.assertIn('folder_error_type', components)


def main():
    suite = unittest.TestLoader().loadTestsFromModule(sys.modules[__name__])
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() else 1


if __name__ == '__main__':
    sys.exit(main())
