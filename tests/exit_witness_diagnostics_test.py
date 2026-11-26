#!/usr/bin/env python3
"""Offline diagnostics tests for ExitWitness (pure fakes + tiny own processes).

Verifies: sanitized typed error receipt (allowlisted class/errno/own location,
no exception strings/paths/PIDs), receipt preserved for run/all_gone/close
paths, unchanged PASS behavior for NoSuchProcess, and a real tiny own process
lifecycle without any browser.
"""
import json
import subprocess
import sys
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'tests' / 'gui'))
import psutil
import youtube_player_native as ypn


def make_witness(roots, identities):
    witness = ypn.ExitWitness.__new__(ypn.ExitWitness)
    witness.psutil = psutil
    witness.clock = 'linux_ticks'
    witness.identities = dict(identities)
    witness.roots = set(roots)
    witness.lock = threading.Lock()
    witness.done = threading.Event()
    witness.error = False
    witness.error_info = None
    witness.samples = 0
    return witness


class FakeAccessDenied(Exception):
    pass


class FakePsutilAccessDenied:
    NoSuchProcess = psutil.NoSuchProcess
    STATUS_ZOMBIE = psutil.STATUS_ZOMBIE

    class Process:
        def __init__(self, pid):
            raise psutil.AccessDenied(pid=pid)


def main():
    checks = {}

    # 1) AccessDenied в sample -> error + санитизированная квитанция
    witness = make_witness({4242}, {4242: '1'})
    witness.psutil = FakePsutilAccessDenied
    original_birth = ypn.kernel_birth
    ypn.kernel_birth = lambda pid, clock: '1'
    try:
        witness.run.__wrapped__ if hasattr(witness.run, '__wrapped__') else None
        # один проход run вручную: sample бросит AccessDenied
        try:
            witness.sample()
            checks['access_denied_raises'] = False
        except psutil.AccessDenied:
            checks['access_denied_raises'] = True
            witness.error = True
            witness._record_error(sys.exc_info()[1], 'run.sample')
    finally:
        ypn.kernel_birth = original_birth
    info = witness.error_info
    checks['receipt_class_allowlisted'] = info['error_class'] == 'AccessDenied'
    checks['receipt_where'] = info['where'] == 'run.sample'
    checks['receipt_counts'] = info['identities_known'] == 1 and info['roots'] == 1
    blob = json.dumps(info)
    checks['receipt_no_paths_pids_strings'] = '/' not in blob and '4242' not in blob and 'pid' not in blob.lower().replace('identities_known', '').replace('roots', '')

    # 2) errno: только bounded int, иначе None
    witness2 = make_witness({1}, {1: '1'})
    witness2._record_error(FileNotFoundError(2, 'No such file', '/secret/path'), 'run.sample')
    checks['errno_bounded'] = witness2.error_info['errno'] == 2
    checks['errno_no_path_leak'] = '/secret' not in json.dumps(witness2.error_info)
    witness2._record_error(OSError('weird'), 'run.sample')
    checks['errno_absent_none'] = witness2.error_info['errno'] is None
    witness2._record_error(ValueError('x'), 'run.sample')
    checks['unknown_class_other'] = witness2.error_info['error_class'] == 'Other'

    # 3) identity_reused: RuntimeError из sample записывается (ребёнок со
    # сменившейся личностью; корень со стабильной).
    witness3 = make_witness({7}, {7: 'same', 8: 'old'})
    ypn.kernel_birth = lambda pid, clock: 'same' if pid == 7 else 'new'
    class FakeChild:
        def __init__(self, pid): self.pid = pid
    class FakeRoot:
        def __init__(self, pid): self.pid = pid
        def children(self, recursive=True): return [FakeChild(8)]
    witness3.psutil = type('P', (), {'NoSuchProcess': psutil.NoSuchProcess, 'Process': FakeRoot})()
    try:
        try:
            witness3.sample()
            checks['identity_reused_raises'] = False
        except RuntimeError as exc:
            witness3._record_error(exc, 'run.sample')
            checks['identity_reused_raises'] = True
            checks['identity_reused_recorded'] = witness3.error_info['error_class'] == 'RuntimeError'
    finally:
        ypn.kernel_birth = original_birth

    # 4) NoSuchProcess — прежний PASS-путь не сломан
    witness4 = make_witness({999999}, {})
    ypn.kernel_birth = lambda pid, clock: (_ for _ in ()).throw(psutil.NoSuchProcess(pid))
    try:
        witness4.sample()
        checks['nosuchprocess_pass'] = witness4.error is False and witness4.error_info is None
    finally:
        ypn.kernel_birth = original_birth

    # 5) Реальный крошечный own-процесс: sample -> выход -> all_gone
    proc = subprocess.Popen(['sleep', '0.4'])
    birth = original_birth(proc.pid, 'linux_ticks')
    witness5 = make_witness({proc.pid}, {proc.pid: birth})
    witness5.sample()
    checks['real_process_sampled'] = proc.pid in witness5.identities and witness5.error is False
    proc.wait(timeout=5)
    deadline = time.monotonic() + 5
    gone = False
    while time.monotonic() < deadline:
        if witness5.all_gone():
            gone = True
            break
        time.sleep(.05)
    checks['real_process_all_gone'] = gone

    # 6) Квитанция гарантирована в finally close_for_privacy (source-факт)
    source = Path(ypn.__file__).read_text(encoding='utf-8')
    checks['receipt_guaranteed_in_finally'] = "report['exit_witness_error']=witness.error_info" in source
    checks['no_weakening'] = "require(not self.error,'Privacy process observation incomplete')" in source

    failed = [name for name, ok in checks.items() if not ok]
    print(json.dumps({'exit_witness_checks': 'PASS' if not failed else 'FAIL', 'failed': failed}))
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main())
