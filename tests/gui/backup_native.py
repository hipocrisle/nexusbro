"""Disposable Linux acceptance: shipped BackupDialog + real GTK pickers, files and crypto.

Synthetic records are seeded with real IPC. All exports/imports run through the
actual UI, actual native file picker and backend; no invoke/fetch/crypto mocks.
Only results and scrubbed logs survive cleanup; never upload the owned data tree.
"""
import argparse
import base64
import hashlib
import json
import os
import pathlib
import re
import secrets
import shutil
import signal
import subprocess
import time

import psutil
from security_native import Device, NativeFailure, call, js_wait, painted, require, settings, super_record, wait_for

DIALOG='section[aria-label="Резервные копии NexusBro"]'


def run(args):
    root=pathlib.Path(args.out).resolve()
    root.mkdir(parents=True,exist_ok=False)
    checks=[];phase='environment';failure=None;device=None
    sensitive=set()
    alarm_before=signal.getsignal(signal.SIGALRM)
    def alarm(_signal,_frame):
        raise NativeFailure('Native backup acceptance exceeded its 360 second deadline')
    signal.signal(signal.SIGALRM,alarm);signal.alarm(360)
    try:
        require(os.environ.get('GITHUB_ACTIONS')=='true','Native backup acceptance requires a disposable GitHub runner')
        exe,cef=pathlib.Path(args.exe).resolve(),pathlib.Path(args.cef).resolve()
        require(exe.is_file() and cef.is_dir(),'Supply the built app and verified CEF directory')
        suffix=secrets.token_hex(8)
        master='native-backup-master-'+suffix
        passphrase='native-independent-export-'+suffix
        wrong='native-wrong-export-'+suffix
        login='native-backup-login-'+suffix
        password='native-backup-password-'+suffix
        proxy_user='native-proxy-user-'+suffix
        proxy_password='native-proxy-password-'+suffix
        route='socks5://'+proxy_user+':'+proxy_password+'@127.0.0.1:9'
        query='native-query-token-'+suffix
        private_name='Native private '+suffix
        private_url='https://private.fixture.invalid/'+suffix
        sensitive.update((master,passphrase,wrong,login,password,proxy_user,proxy_password,route,query,private_name,private_url))
        device=Device('backup',root/'device',exe,cef)
        keeper=super_record('backup-live','Native backup live','about:blank')
        source=super_record('backup-source','Native backup source','https://source.fixture.invalid/path?token='+query+'#'+query,proxy=route)
        target=super_record('backup-target','Native backup target','about:blank')
        private=super_record('legacy-private',private_name,private_url,private=True)
        device.data.joinpath('vkladki.json').write_text(json.dumps([keeper,source,target,private]))
        device.data.joinpath('aktivnaya.json').write_text(json.dumps('backup-live'))
        device.data.joinpath('sohranennye.json').write_text('[]')
        config=settings('temnaya','backup-local-route')
        config['vyhody']=[{'id':'backup-local-route','nazvanie':'Native local route','adres':route,'proxy_uzel':''}]
        device.data.joinpath('nastrojki.json').write_text(json.dumps(config))
        phase='start-and-seed'
        client=device.start()
        js_wait(client,'!!document.querySelector("button[title=\\"Проверить обновления\\"]")','Shipped App did not mount',30)
        call(client,'security_vault',action='create',password=master)
        call(client,'security_passwords',action='save',data={'super_id':'backup-source','origin':'https://login.fixture.invalid','username':login,'password':password,'title':'Native backup credential'})
        call(client,'zakladki_sohranit',adres='https://bookmark.fixture.invalid/path?auth='+query+'#'+query,nazvanie='Native backup bookmark',papka='Native backup folder')
        before_rows=call(client,'security_passwords',action='list')
        require(len(before_rows)==1,'Synthetic vault did not contain exactly one source credential')
        env={**os.environ,'DISPLAY':device.display}

        def xdo(*arguments,check=True):
            result=subprocess.run(['xdotool',*map(str,arguments)],env=env,capture_output=True,text=True,timeout=6)
            if check:require(result.returncode==0,'Native X11 input command failed')
            return result

        def app_pids():
            try:children=psutil.Process(device.app.pid).children(recursive=True)
            except psutil.NoSuchProcess:return set()
            out=set()
            for process in children:
                try:
                    if pathlib.Path(process.exe()).resolve()==exe:out.add(process.pid)
                except (psutil.NoSuchProcess,psutil.AccessDenied):pass
            return out

        def owned_windows():
            found=set()
            for pid in app_pids():
                result=xdo('search','--onlyvisible','--pid',pid,check=False)
                if result.returncode==0:found.update(line for line in result.stdout.splitlines() if line.isdigit())
            return found

        def focus_shell():
            call(client,'fokus_obolochke')
            label=client.js('window.__TAURI_INTERNALS__.metadata.currentWindow.label')
            title=client.command('plugin:window|title',label=label)
            windows=owned_windows()
            matches=[w for w in windows if xdo('getwindowname',w,check=False).stdout.strip()==title]
            require(matches,'Owned NexusBro shell XID was not found')
            xdo('windowfocus','--sync',matches[0]);painted(client)

        def button(text):
            selector=DIALOG+' button[data-native-backup-button]'
            found=client.js('(()=>{const d=document.querySelector('+json.dumps(DIALOG)+');if(!d)return false;for(const n of d.querySelectorAll("[data-native-backup-button]"))n.removeAttribute("data-native-backup-button");const b=[...d.querySelectorAll("button")].find(b=>b.textContent.trim()==='+json.dumps(text)+');if(!b||b.disabled)return false;b.setAttribute("data-native-backup-button","");b.scrollIntoView({block:"center"});return true})()')
            require(found,'Shipped backup button is absent or disabled: '+text)
            focus_shell();client.click(selector)

        def alert_contains(text,timeout=15):
            js_wait(client,'(()=>{const e=document.querySelector('+json.dumps(DIALOG+' [role="alert"]')+');return !!e&&e.innerText.includes('+json.dumps(text)+')})()','Backup error was not shown',timeout)

        def status_contains(text,timeout=20):
            js_wait(client,'(()=>{const e=document.querySelector('+json.dumps(DIALOG+' [role="status"]')+');return !!e&&e.innerText.includes('+json.dumps(text)+')})()','Backup completion was not shown',timeout)

        def choose(text,path,save=False):
            # These operations never select outside the test-owned output tree.
            path=path.resolve()
            require(root in path.parents,'Native picker path escaped the owned test root')
            before=owned_windows();button(text)
            new=wait_for(lambda:owned_windows()-before,'Native GTK file chooser did not appear as an owned app window',15)
            require(len(new)==1,'Expected exactly one new native file chooser window')
            xid=next(iter(new));xdo('windowfocus','--sync',xid)
            xdo('key','--clearmodifiers','ctrl+l')
            xdo('type','--clearmodifiers','--delay','2','--',str(path))
            xdo('key','--clearmodifiers','Return')
            # GTK save dialogs can resolve the Location entry before accepting.
            deadline=time.monotonic()+2
            while time.monotonic()<deadline and xid in owned_windows():time.sleep(.1)
            if xid in owned_windows():
                xdo('windowfocus','--sync',xid)
                xdo('key','--clearmodifiers','alt+s' if save else 'alt+o')
            wait_for(lambda:xid not in owned_windows(),'Native GTK file chooser did not accept the selected owned path',12)
            focus_shell()

        def open_backup():
            client.js('window.dispatchEvent(new Event("nexusbro-backup"));true')
            js_wait(client,'!!document.querySelector('+json.dumps(DIALOG)+')','Actual BackupDialog did not open',10)
            painted(client)

        def select(label,text,occurrence=0):
            selector=DIALOG+' button[role="combobox"][aria-label='+json.dumps(label)+']'
            require(client.js('(()=>{const e=document.querySelector('+json.dumps(selector)+');if(!e)return false;e.scrollIntoView({block:"center"});return true})()'),'Backup mapping selector is missing')
            focus_shell();client.click(selector)
            js_wait(client,'!!document.querySelector(".vybor-popup")','Custom mapping popup did not open',5)
            found=client.js('(()=>{const row=[...document.querySelectorAll(".vybor-popup [role=option]")].filter(x=>x.textContent.trim()==='+json.dumps(text)+')['+str(occurrence)+'];if(!row||row.disabled)return false;row.setAttribute("data-native-backup-option","");row.scrollIntoView({block:"nearest"});return true})()')
            require(found,'Expected explicit mapping option is missing')
            client.click('.vybor-popup [data-native-backup-option]')
            js_wait(client,'!document.querySelector(".vybor-popup")','Mapping popup did not close',5)

        def fill(label,value):
            selector=DIALOG+' input[aria-label='+json.dumps(label)+']'
            require(client.js('(()=>{const e=document.querySelector('+json.dumps(selector)+');if(!e)return false;e.scrollIntoView({block:"center"});return true})()'),'Backup password field is missing')
            focus_shell();client.fill(selector,value)

        def target_hashes():
            names=['nastrojki.json','zakladki.json','sohranennye.json','security/browser-vault.v1']
            return {name:hashlib.sha256(device.data.joinpath(name).read_bytes()).hexdigest() if device.data.joinpath(name).exists() else None for name in names}

        def passwords_clear():
            require(client.js('(()=>{const d=document.querySelector('+json.dumps(DIALOG)+');return d&&[...d.querySelectorAll("input[type=password]")].every(e=>!e.value)})()'),'Backup password fields retained a secret')

        phase='native-config-export'
        open_backup()
        exported=root/'portable-config.json'
        choose('Сохранить настройки в файл',exported,save=True)
        status_contains('Копия настроек сохранена')
        require(exported.is_file(),'Actual native config export did not create its selected file')
        raw=exported.read_bytes();portable=json.loads(raw)
        require(portable.get('format')=='nexusbro-portable-config' and portable.get('version')==1,'Config export has wrong format')
        require(len(portable['supers'])==3 and not any(s['id'].startswith('pvt-') or s['name']==private_name for s in portable['supers']),'Config export included a private Super')
        require(not {'vyhody','proxy','account','passwords'}.intersection(portable['settings']),'Config export included nonportable settings')
        require(all(marker.encode() not in raw for marker in sensitive),'Config export leaked synthetic credentials or private metadata')
        require(portable['bookmarks'][0]['url']=='https://bookmark.fixture.invalid/path','Config export retained signed query/fragment or lost bookmark structure')
        checks.append({'native_picker_real_config_export_whitelist_private_proxy_and_url_secret_exclusion':True})

        phase='native-config-preview-and-import'
        before=target_hashes();original_ids={r['id'] for r in call(client,'spisok')}
        choose('Импортировать настройки',exported)
        js_wait(client,'document.body.innerText.includes("Предпросмотр импорта")','Real config file was not previewed')
        require(target_hashes()==before,'Config preview changed persistent targets before confirmation')
        require(client.js('(()=>{const b=[...document.querySelectorAll('+json.dumps(DIALOG+' button')+')].find(b=>b.textContent.trim()==="Подтвердить импорт");return b&&b.disabled})()'),'Import was enabled before all routes were selected')
        for s in portable['supers']:
            select('Путь импортируемой супервкладки: '+s['name'],'Native local route' if s['id']=='backup-source' else 'Прямое соединение')
        require(target_hashes()==before,'Selecting import routes mutated persistent targets')
        button('Подтвердить импорт');status_contains('Резервная копия импортирована')
        saved=json.loads(device.data.joinpath('sohranennye.json').read_text())
        require(len(saved)==3 and all(s['id'] not in original_ids for s in saved),'Config import did not create three independent saved IDs')
        require(all(not s.get('privatnaya') for s in saved),'Config import created private metadata')
        source_import=next(s for s in saved if s['nazvanie']=='Native backup source')
        require(source_import['proxy']==route,'Explicit selected local route was not preserved')
        require(all(s.get('proxy','')=='' for s in saved if s['id']!=source_import['id']),'Explicit Direct choices were not preserved')
        require(not any(s['id'] in {r['id'] for r in call(client,'spisok')} for s in saved),'Saved config imports auto-opened Supers')
        require(not any(device.data.joinpath('profili',s['id']).exists() for s in saved),'Config import started profile/CEF writers')
        require(not device.data.joinpath('browser-backup-import.v1').exists(),'Successful config import left an incomplete transaction journal')
        checks.append({'native_config_preview_read_only_explicit_routes_confirmed_new_ids_saved_without_engine':True})

        phase='native-encrypted-password-export'
        archive=root/'passwords.encrypted.json'
        fill('Пароль резервной копии',passphrase);fill('Повторите пароль резервной копии',passphrase)
        choose('Сохранить зашифрованные пароли',archive,save=True)
        status_contains('Зашифрованная копия паролей сохранена');passwords_clear()
        archive_bytes=archive.read_bytes();envelope=json.loads(archive_bytes)
        require(envelope.get('format')=='nexusbro-password-backup' and envelope.get('memory_kib')==19456 and envelope.get('iterations')==2 and envelope.get('lanes')==1,'Password archive did not use the bounded authenticated format')
        require(all(marker.encode() not in archive_bytes for marker in (login,password,passphrase,master,proxy_user,proxy_password)),'Password archive contains a plaintext synthetic secret')
        checks.append({'native_picker_real_password_export_separate_passphrase_ciphertext_and_secret_field_clear':True})

        phase='wrong-password-no-mutation'
        before=target_hashes()
        fill('Пароль резервной копии',wrong)
        choose('Открыть копию паролей',archive)
        alert_contains('Неверный пароль резервной копии')
        require(target_hashes()==before,'Wrong archive password changed local targets')
        phase='tampered-password-archive-no-mutation'
        tampered=root/'tampered.encrypted.json';edited=json.loads(archive_bytes)
        payload=bytearray(base64.b64decode(edited['ciphertext']));payload[0]^=1;edited['ciphertext']=base64.b64encode(payload).decode();tampered.write_text(json.dumps(edited))
        fill('Пароль резервной копии',passphrase)
        choose('Открыть копию паролей',tampered)
        alert_contains('Неверный пароль резервной копии')
        require(target_hashes()==before,'Authenticated tamper rejection changed local targets')
        checks.append({'native_wrong_passphrase_and_tampered_ciphertext_rejected_without_target_mutation':True})

        phase='actual-password-preview-map-and-commit'
        fill('Пароль резервной копии',passphrase)
        choose('Открыть копию паролей',archive)
        js_wait(client,'document.body.innerText.includes("Предпросмотр импорта")','Actual encrypted backup did not decrypt for preview')
        require(target_hashes()==before,'Password preview changed local targets')
        # Full config roundtrip intentionally produces duplicate display names.
        # Select the known original target by its position in the actual sorted
        # backend catalog, with physical input; do not replace the UI mapping.
        catalog={s['id']:s for s in saved}
        catalog.update({r['id']:r for r in call(client,'spisok') if not r.get('privatnaya') and not r['id'].startswith('pvt-')})
        matching_ids=sorted(id for id,row in catalog.items() if row['nazvanie']=='Native backup target')
        require('backup-target' in matching_ids,'Original mapped target Super disappeared')
        select('Куда импортировать пароли: Native backup source','Native backup target',matching_ids.index('backup-target'))
        button('Подтвердить импорт');status_contains('Резервная копия импортирована');passwords_clear()
        rows=call(client,'security_passwords',action='list')
        imported=[r for r in rows if r.get('super_id')=='backup-target']
        require(len(rows)==2 and len(imported)==1,'Password import did not add exactly one mapped target record')
        require(imported[0]['id']!=before_rows[0]['id'] and imported[0]['username']==login,'Imported credential ID or metadata is incorrect')
        revealed=call(client,'security_passwords',action='reveal',data={'id':imported[0]['id']})
        require(revealed==password,'Native vault did not recover the actual imported password')
        del revealed
        vault_bytes=device.data.joinpath('security/browser-vault.v1').read_bytes()
        require(password.encode() not in vault_bytes and login.encode() not in vault_bytes,'Imported credential was persisted in plaintext')
        require(client.js('localStorage.length===0&&sessionStorage.length===0'),'Backup UI persisted plaintext browser storage')
        checks.append({'native_actual_decrypt_prepare_mapped_password_import_new_id_vault_roundtrip_ciphertext':True})
        phase='complete'
        checks.append({'success':True,'native_crypto_file_io_and_gtk_pickers':'actual','ipc_overrides':False,'platform':'linux'})
    except Exception as exc:
        message=str(exc)
        for secret in sorted(sensitive,key=len,reverse=True):message=message.replace(secret,'[synthetic secret redacted]')
        message=message.replace(str(root),'[owned test directory]')
        failure='backup_native failed during '+phase+': '+message[:1200]
        checks.append({'success':False,'failed_phase':phase,'error':message[:1200]})
    finally:
        signal.alarm(0);signal.signal(signal.SIGALRM,alarm_before)
        if device:
            try:device.close()
            except Exception:
                failure=failure or 'Native backup process cleanup failed'
                checks.append({'success':False,'cleanup_failed':True})
        # Persist only fixed results and scrubbed logs. Owned fixtures contain
        # synthetic credentials by design and must not become public artifacts.
        for logfile in root.rglob('*.log'):
            text=logfile.read_text(errors='replace')
            for secret in sorted(sensitive,key=len,reverse=True):text=text.replace(secret,'[synthetic secret redacted]')
            logfile.write_text(text)
        for path in root.iterdir():
            if path.name=='device' and path.is_dir():
                for child in path.iterdir():
                    if child.is_dir():shutil.rmtree(child)
                    elif child.suffix!='.log':child.unlink()
            elif path.is_file() and path.suffix=='.json':path.unlink()
        root.joinpath('results.json').write_text(json.dumps(checks,indent=2)+'\n')
        print(json.dumps(checks))
    if failure:raise NativeFailure(failure)


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--exe',required=True)
    parser.add_argument('--cef',required=True)
    parser.add_argument('--out',required=True)
    run(parser.parse_args())
