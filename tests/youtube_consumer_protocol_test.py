"""Exercise the compiled non-diagnostic probe against the actual consumer parser.

GHA-only protocol fixture: the owned extractor prints a constant error and
exits. It performs no extraction, playback, or network operation.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile


def check(probe):
    assert os.environ.get('GITHUB_ACTIONS') == 'true' and sys.platform == 'linux', 'gha_linux_only'
    assert probe.is_file() and not probe.is_symlink(), 'probe_missing'
    probe = probe.resolve(strict=True)
    module_path = Path(__file__).parent / 'gui' / 'youtube_extract_consumer.py'
    spec = importlib.util.spec_from_file_location('consumer_protocol', module_path)
    consumer = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(consumer)
    with tempfile.TemporaryDirectory(prefix='nb-consumer-protocol-') as temporary:
        root = Path(temporary)
        extractor = root / 'yt-dlp'
        extractor.write_text("#!/bin/sh\nprintf '%s\\n' \"ERROR: Sign in to confirm you're not a bot\" >&2\nexit 1\n")
        runtime = root / 'deno'
        runtime.write_text('#!/bin/sh\nexit 1\n')
        extractor.chmod(0o700)
        runtime.chmod(0o700)
        process = subprocess.Popen([str(probe), str(root), 'http://127.0.0.1:1', 'aqz-KE-bpKQ'],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            start_new_session=True, env={'GITHUB_ACTIONS': 'true', 'PATH': os.defpath})
        try:
            stdout, stderr = process.communicate(b'GO\n', timeout=10)
        except subprocess.TimeoutExpired:
            if process.poll() is None:
                try: os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError: pass
            process.communicate(timeout=3)
            raise AssertionError('protocol_fixture_timeout') from None
        assert process.returncode == 3 and len(stdout) <= 65536 and len(stderr) <= 4096, 'protocol_fixture_failed'
        parsed = consumer.summary(stdout)
        assert parsed['stage'] == 'extractor' and parsed['ok'] is False, 'protocol_error_state'
        assert parsed['reason'] == 'ChallengeRequired', 'protocol_reason'
        assert 'challenge_subreason' not in json.loads(stdout), 'ordinary_schema_changed'
    print(json.dumps({'protocol_compatible': True, 'public_network_requests': 0,
                      'extractor_acceptance': False, 'playback_tested': False}))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--probe-exe', type=Path, required=True)
    check(parser.parse_args().probe_exe)
