#!/usr/bin/env python3
"""Contract test for downloads open/folder policy — REAL production function.

Compiles the actual production module desktop/src-tauri/src/downloads_open_policy.rs
with `rustc --test` (tiny single-file build, not a project build) and runs its
unit tests against real temp files. Also asserts the wiring contract in
zagruzki.rs. No app/browser execution.
"""
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
POLICY = ROOT / 'desktop/src-tauri/src/downloads_open_policy.rs'
ZAGRUZKI = ROOT / 'desktop/src-tauri/src/zagruzki.rs'


def main():
    report = {'schema': 1, 'production_policy_tests': None, 'wiring': {}, 'skipped': None}
    rustc = shutil.which('rustc')
    if not rustc:
        report['skipped'] = 'rustc_unavailable'
        print(json.dumps(report))
        return 1
    with tempfile.TemporaryDirectory(prefix='nb-policy-test-') as tmp:
        binary = Path(tmp) / 'policy_test'
        compile_run = subprocess.run(
            [rustc, '--test', '--edition', '2021', str(POLICY), '-o', str(binary)],
            capture_output=True, text=True, timeout=300)
        if compile_run.returncode != 0:
            report['production_policy_tests'] = 'compile_failed'
            print(json.dumps(report))
            return 1
        run = subprocess.run([str(binary)], capture_output=True, text=True, timeout=120)
        passed = 'test result: ok' in run.stdout and '0 failed' in run.stdout
        report['production_policy_tests'] = 'PASS' if passed else 'FAIL'
        report['policy_test_tail'] = run.stdout.strip().splitlines()[-1] if run.stdout.strip() else ''
    src = ZAGRUZKI.read_text(encoding='utf-8')
    report['wiring'] = {
        'uses_policy_open': 'crate::downloads_open_policy::check_open_target' in src,
        'uses_policy_folder': 'crate::downloads_open_policy::check_folder_target' in src,
        'no_inline_danger_list_left': 'DANGEROUS' not in src,
        'neutral_spawn_errors': 'map_err(|_|"Не удалось открыть' in src,
    }
    ok = report['production_policy_tests'] == 'PASS' and all(report['wiring'].values())

    # --- Реальный localhost-прогон собственной Fixture (без браузера) ---
    sys.path.insert(0, str(ROOT / 'tests' / 'gui'))
    import urllib.request
    import downloads_library_native as dln
    fixture = dln.Fixture()
    origin = fixture.start()
    progress = {}
    try:
        with urllib.request.urlopen(origin + '/file-known.bin', timeout=30) as response:
            progress['known_headers'] = (response.headers.get('Content-Length') == str(dln.KNOWN_TOTAL)
                                         and 'attachment' in (response.headers.get('Content-Disposition') or '')
                                         and 'file-known.bin' in response.headers.get('Content-Disposition'))
            body = response.read()
        progress['known_bytes_exact'] = len(body) == dln.KNOWN_TOTAL
        with urllib.request.urlopen(origin + '/file-unknown.bin', timeout=30) as response:
            progress['unknown_no_length'] = response.headers.get('Content-Length') is None
            body = response.read()
        progress['unknown_bytes_exact'] = len(body) == dln.KNOWN_TOTAL
        with urllib.request.urlopen(origin + '/note.txt', timeout=10) as response:
            progress['doc_bytes'] = len(response.read()) == 5
    finally:
        fixture.close()

    # --- FakeClient: sanitize-снимок и typed-коды (не rg-only) ---
    class FakeClient:
        def __init__(self, records): self.records = records
    fake = FakeClient([
        {'name': 'file-known.bin', 'status': 'Загружается', 'received': 65536, 'total': dln.KNOWN_TOTAL,
         'percent': 8, 'speed': 65536, 'url': 'http://secret/url', 'path': '/secret/path', 'id': 'x:1:2'},
        {'name': 'чужое-имя.bin', 'status': 'Неведомый', 'received': float('nan'), 'total': -5,
         'percent': 10 ** 15, 'speed': None},
        'not-a-dict',
    ])
    original_call = dln.call
    dln.call = lambda client, command, **kw: client.records
    try:
        summary = dln.download_summary(fake)
    finally:
        dln.call = original_call
    blob = json.dumps(summary, ensure_ascii=False)
    progress['summary_available'] = summary['available'] is True and summary['count'] == 2
    progress['summary_kind_enum'] = [r['file_kind'] for r in summary['records']] == ['known', 'other']
    progress['summary_status_enum'] = [r['status'] for r in summary['records']] == ['Загружается', 'other']
    progress['summary_scalars_bounded'] = summary['records'][1]['received'] is None \
        and summary['records'][1]['total'] is None and summary['records'][1]['percent'] is None
    progress['summary_no_raw_fields'] = 'secret' not in blob and 'http' not in blob \
        and 'чужое' not in blob
    known_codes = {'known_progress_timeout', 'unknown_total_timeout',
        'received_stall_timeout', 'doc_complete_timeout', 'known_complete_timeout',
        'unknown_complete_timeout', 'library_open_timeout'}
    progress['wait_messages_mapped'] = known_codes.issubset(set(dln.WAIT_MESSAGES.values())) \
        and all(isinstance(code, str) and code.islower() for code in dln.WAIT_MESSAGES.values())
    native_src = (ROOT / 'tests/gui/downloads_library_native.py').read_text(encoding='utf-8')
    progress['native_failure_caught'] = 'WAIT_MESSAGES.get(message' in native_src \
        and "'native_assertion'" in native_src
    progress['snapshot_before_and_failure'] = "report['downloads_before_wait']" in native_src \
        and "report['downloads_at_failure']" in native_src
    progress['downloads_serialized'] = 'time.sleep(.3)' not in native_src
    progress['preparation_false_on_real_run'] = "report['preparation'] = False" in native_src
    progress['shim_still_not_os_proof'] = "'open_launch': 'test_shim_only'" in native_src

    # --- Trusted page click: счётчики fixture, чистая геометрия/цвет/url_kind ---
    import download_page_input as dpi
    fixture2 = dln.Fixture()
    origin2 = fixture2.start()
    try:
        with urllib.request.urlopen(origin2 + '/page.html', timeout=10) as response:
            page = response.read().decode('utf-8')
        progress['page_counters'] = fixture2.counts()['page'] == 1
        for kind, path in (('doc', '/note.txt'), ('known', '/file-known.bin'), ('unknown', '/file-unknown.bin')):
            urllib.request.urlopen(origin2 + path, timeout=30).read()
        counts = fixture2.counts()
        progress['attachment_counters'] = counts['doc'] == 1 and counts['known'] == 1 and counts['unknown'] == 1
    finally:
        fixture2.close()
    progress['tiles_in_page'] = all(f'id="{k}"' in page and f'#{k}' in page for k in ('doc', 'known', 'unknown')) \
        and 'background:#123456' in page
    centers = [dpi.tile_center(k) for k in ('doc', 'known', 'unknown')]
    progress['tile_centers_fixed'] = centers == [(140, 80), (140, 200), (140, 320)]
    progress['tile_centers_distinct'] = len({c[1] for c in centers}) == 3
    origin_mock = {'x': 100, 'y': 50}
    pole_mock = {'x': 10, 'y': 200, 'w': 900, 'h': 600}
    progress['screen_point_math'] = dpi.screen_point(origin_mock, pole_mock, (140, 80), 1.0) == (250, 330)
    progress['screen_point_scale'] = dpi.screen_point(origin_mock, pole_mock, (140, 80), 1.5) == (325, 470)
    progress['color_match_positive'] = dpi.color_matches((0xC0, 0x20, 0x20), dpi.TILES['doc']['rgb']) is True
    progress['color_match_negative'] = dpi.color_matches((0x20, 0xC0, 0x20), dpi.TILES['doc']['rgb']) is False \
        and dpi.color_matches((0x12, 0x34, 0x56), dpi.TILES['known']['rgb']) is False
    progress['url_kind_mapping'] = (
        dpi.url_kind(origin2 + '/page.html', origin2) == 'page'
        and dpi.url_kind(origin2 + '/file-known.bin', origin2) == 'known'
        and dpi.url_kind('https://other.invalid/x', origin2) == 'other'
        and dpi.url_kind(None, origin2) == 'other')
    session_mock = {'vkladki': [{'id': 1, 'browser_id': 5, 'zagruzka': False, 'route_pending': False,
                                 'route_error': None, 'adres': origin2 + '/page.html'}]}
    facts = dpi.tab_facts(session_mock, 1, origin2)
    progress['tab_facts_typed'] = facts == {'browser_positive': True, 'zagruzka': False,
        'route_pending': False, 'route_error_present': False, 'url_kind': 'page'}
    progress['no_pereyti_downloads'] = "origin + '/note.txt'" not in native_src \
        and "origin + '/file-known.bin'" not in native_src and "origin + '/file-unknown.bin'" not in native_src
    progress['trusted_click_wired'] = "trusted_download('doc', 'doc')" in native_src \
        and "trusted_download('known', 'known')" in native_src \
        and "trusted_download('unknown', 'unknown')" in native_src
    progress['counter_proof_required'] = 'http_branch_reached' in native_src \
        and 'did not reach the fixture HTTP branch' in native_src
    progress['utc_timestamp'] = 'datetime.now(timezone.utc)' in native_src

    # --- Startup facts: pure parser (секреты не выходят, malformed typed) ---
    import download_startup_facts as dsf
    log = (
        'noise line with http://secret.url/page?token=abc\n'
        'NEXUSBRO_MEMORY_BOOTSTRAP stage=observer_ready browser=17\n'
        'NEXUSBRO_MEMORY_BOOTSTRAP browser=17 registered=true safety=early\n'
        'NEXUSBRO_MEMORY_BOOTSTRAP stage=deadline browser=18\n'
        'NEXUSBRO_MEMORY_BOOTSTRAP stage=weird_stage browser=19\n'
        'NEXUSBRO_MEMORY_DOCUMENT browser=17 accepted=true\n'
        'NEXUSBRO_MEMORY_DOCUMENT browser=18 accepted=false nonce=deadbeefcafe\n'
        'cef: tab 3 renderer terminated code=-5\n'
        'NEXUSBRO_MEMORY_BOOTSTRAP stage=deadline browser=NOT_A_NUMBER\n'
    )
    facts = dsf.parse_native_facts(log)
    kinds = [(e['kind'], e.get('stage'), e.get('registered'), e.get('accepted')) for e in facts['events']]
    progress['parser_events'] = kinds == [
        ('bootstrap', 'observer_ready', None, None),
        ('bootstrap', 'finish', True, None),
        ('bootstrap', 'deadline', None, None),
        ('bootstrap', 'other', None, None),
        ('document', None, None, True),
        ('document', None, None, False),
        ('renderer_terminated', None, None, None),
    ]
    blob = json.dumps(facts)
    progress['parser_no_secrets'] = 'secret.url' not in blob and 'deadbeef' not in blob \
        and 'token' not in blob and 'nonce' not in blob
    progress['parser_malformed_ignored'] = len(facts['events']) == 7  # NOT_A_NUMBER строка отброшена
    progress['parser_lines_seen'] = facts['lines_seen'] == 9 and facts['no_events'] is False
    empty = dsf.parse_native_facts('')
    progress['parser_no_events_explicit'] = empty['no_events'] is True and empty['lines_seen'] == 0
    progress['parser_lines_cap'] = dsf.parse_native_facts('x\n' * 5000)['lines_seen'] == dsf.MAX_LINES
    # collect_startup_facts: available/readable вместо 0=PASS
    import tempfile as _tf
    class FakeDevice:
        pass
    fd = FakeDevice()
    with _tf.TemporaryDirectory() as tmp:
        fd.root = tmp
        fd.data = tmp + '/data'
        import pathlib as _pl
        (_pl.Path(tmp) / 'app-1.log').write_text('NEXUSBRO_MEMORY_BOOTSTRAP stage=deadline browser=1\n')
        (_pl.Path(fd.data) / 'profili' / 'downloads').mkdir(parents=True)
        (_pl.Path(fd.data) / 'profili' / 'downloads' / 'cef.log').write_text('')
        collected = dsf.collect_startup_facts(fd, 'downloads')
    progress['collect_app_event'] = collected['app']['available'] and collected['app']['events'][0]['stage'] == 'deadline'
    progress['collect_engine_empty_explicit'] = collected['engine']['available'] is True \
        and collected['engine']['no_events'] is True and collected['engine']['lines_seen'] == 0
    # owned screenshot: source-факты строгости
    shot_src = (ROOT / 'tests/gui/download_startup_facts.py').read_text(encoding='utf-8')
    progress['shot_identity_before_after'] = shot_src.count('identity_check()') >= 2
    progress['shot_own_display_only'] = 'xdisplay=device.display' in shot_src
    progress['shot_size_capped'] = '3 * 1024 * 1024' in shot_src
    progress['shot_no_second_inspector'] = 'inspector' not in shot_src
    progress['shot_wired_on_failure_only'] = "if not report.get('completed')" in native_src \
        and "report['owned_failure_capture']" in native_src
    progress['startup_facts_wired'] = "report['startup_facts'] = collect_startup_facts(device, 'downloads')" in native_src
    report['progress_facts'] = progress
    ok = ok and all(progress.values())
    print(json.dumps(report, ensure_ascii=False))
    return 0 if ok else 1


if __name__ == '__main__':
    sys.exit(main())
