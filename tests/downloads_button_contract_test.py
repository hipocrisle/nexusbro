#!/usr/bin/env python3
"""Offline contract checks for DownloadsButton (no UI/browser execution)."""
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
tsx = (ROOT / 'desktop/src/DownloadsButton.tsx').read_text(encoding='utf-8')
css = (ROOT / 'desktop/src/downloadsButton.css').read_text(encoding='utf-8')

checks = {}
# Обновление и throttle
checks['no_idle_poll_timer'] = 'setInterval' not in tsx
checks['mount_refresh'] = 'refresh();' in tsx and 'useEffect' in tsx
checks['event_invalidate'] = "listen<unknown>('downloads-changed'" in tsx
checks['throttle_4hz'] = '250' in tsx and 'lastRun' in tsx
checks['stale_generation_guard'] = 'generation.current!==gen' in tsx and 'generation.current++' in tsx
checks['cleanup_listeners_timers'] = 'clearTimeout' in tsx and 'off.then(f=>f())' in tsx
# Точный фильтр и приватность
checks['exact_super_and_private_filter'] = 'd.super_id===superId&&d.privatnaya===privatnaya' in tsx
checks['badge_counts_only'] = 'downloads-badge' in tsx and '{active.length}' in tsx
forbidden = ['d.url', 'd.path', 'd.name', 'LocationURL']
checks['no_names_urls_paths'] = not any(token in tsx for token in forbidden)
checks['tooltip_counts_only'] = 'активных ${active.length}' in tsx and 'http' not in tsx
# Прогресс и статусы
checks['weighted_only_all_known'] = 'active.every(d=>d.total>0&&d.percent>=0)' in tsx
checks['indeterminate_when_unknown'] = "weighted===null?' indeterminate':''" in tsx
checks['paused_distinct_not_distorted'] = "' paused'" in tsx and 'Приостановлена' in tsx
checks['opens_panel_via_onopen'] = 'onClick={onOpen}' in tsx
checks['no_navigation_replacement'] = 'pereyti' not in tsx and 'novaya' not in tsx
# Стиль
checks['dark_only'] = '#fff;' not in css and 'background:#fff' not in css
checks['no_heavy_animation'] = '@keyframes' not in css and 'animation' not in css
checks['no_global_busy'] = 'aria-busy' not in tsx

failed = [name for name, ok in checks.items() if not ok]
print(json.dumps({'downloads_button_checks': 'PASS' if not failed else 'FAIL', 'failed': failed}))
sys.exit(1 if failed else 0)
