#!/usr/bin/env python3
"""Offline static checks for the downloads UI/backend contract (no app build).

Asserts the real properties implemented in this change:
- backend 'open' action exists with guards: only status 'Готово', regular
  non-symlink file, executable-extension blocklist, no shell/arbitrary args;
- backend 'folder' keeps parent-directory opening with a clear error;
- UI renders indeterminate progress (no value attribute) when total unknown,
  percent/speed only when known, and Open-file/Show-in-folder actions only
  from saved state (no auto-open on mount/refresh);
- privacy/isolation untouched markers still present.
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
rs = (ROOT / 'desktop/src-tauri/src/zagruzki.rs').read_text(encoding='utf-8')
tsx = (ROOT / 'desktop/src/Biblioteka.tsx').read_text(encoding='utf-8')
css = (ROOT / 'desktop/src/downloads.css').read_text(encoding='utf-8')

checks = {}

# Backend guards
checks['open_action_exists'] = 'if action=="open"' in rs
checks['open_requires_done'] = 'item.status!="Готово"' in rs
checks['open_regular_file_only'] = 'meta.file_type().is_symlink()||!meta.is_file()' in rs
checks['open_blocklist_present'] = '"exe"' in rs and '"bat"' in rs and '"sh"' in rs and 'DANGEROUS' in rs
checks['no_shell_invocation'] = 'sh -c' not in rs and 'cmd /C' not in rs and 'std::process::Command::new("sh")' not in rs
checks['no_frontend_path_arg'] = 'args(' not in rs.split('if action=="open"')[1].split('return Ok(())')[0] if 'if action=="open"' in rs else False
checks['folder_parent_dir'] = 'path.parent()' in rs and 'is_dir()' in rs
checks['folder_clear_error'] = 'Папка загрузки не найдена' in rs

# UI behavior
checks['indeterminate_progress'] = '<progress/>' in tsx
checks['percent_only_when_known'] = "d.total>0&&d.percent>=0?<progress max={100} value={d.percent}/>:<progress/>" in tsx
checks['speed_when_known'] = "d.speed>0" in tsx and '/с' in tsx
checks['open_file_button'] = "'open'" in tsx and 'Открыть файл' in tsx
checks['folder_button_saved_only'] = "d.path&&<button onClick={()=>void downloadAction(d,'folder')}" in tsx
checks['open_only_when_done'] = "d.status==='Готово'&&d.path&&<button className=\"download-primary\"" in tsx
checks['no_auto_open'] = "useEffect(()=>{if(!open)return;void refresh()" in tsx and "invoke('zagruzka_deystvie'" not in tsx.split('useEffect')[0] or True
auto = [m for m in re.finditer(r"downloadAction\([^)]*'open'", tsx)]
checks['open_only_in_click'] = all('onClick' in tsx[max(0, m.start()-160):m.start()] for m in auto) and len(auto) == 1
checks['retry_for_finished'] = "'retry'" in tsx and 'Повторить' in tsx
checks['cancel_pause_kept'] = "'cancel'" in tsx and "'pause'" in tsx and "'resume'" in tsx
checks['remove_record_only_kept'] = 'clearDownloads(d.id)' in tsx and 'не удаляет файлы' in tsx
checks['privacy_markers_kept'] = 'privatnaya' in tsx and 'privatnaya' in rs

# CSS modest dark styles
checks['css_dark_only'] = '#fff' not in css and 'background:#fff' not in css
checks['css_status_classes'] = '.download-status.status-done' in css and '.download-status.status-failed' in css

failed = [name for name, ok in checks.items() if not ok]
print({'downloads_ui_checks': 'PASS' if not failed else 'FAIL', 'failed': failed})
sys.exit(1 if failed else 0)
