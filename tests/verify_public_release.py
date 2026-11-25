"""Anonymous post-publication crypto verification. Payload traffic is GHA-only.

This does not publish, install, execute, or unpack packages. Publisher-provided
local expectations pin the candidate; only small public receipts leave the job.
"""
import argparse
import base64
import hashlib
import io
import json
import os
from pathlib import Path
import re
import ssl
import signal
import threading
from contextlib import contextmanager
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey

REPO = 'hipocrisle/nexusbro'
HOSTS = {'github.com', 'release-assets.githubusercontent.com', 'objects.githubusercontent.com'}
META_LIMIT = 256 * 1024
SIG_LIMIT = 16 * 1024
PAYLOAD_LIMIT = 2 * 1024 ** 3
PLATFORMS = {'.exe': 'windows-x86_64', '.deb': 'linux-x86_64-deb', '.rpm': 'linux-x86_64-rpm'}


class Refused(Exception):
    def __init__(self, code): self.code = code


def require(condition, code):
    if not condition: raise Refused(code)


def admit(environment):
    require(environment.get('GITHUB_ACTIONS') == 'true', 'not_github_actions')


class Deadline:
    def __init__(self, seconds=900): self.end = time.monotonic() + seconds
    def timeout(self):
        remaining = self.end - time.monotonic()
        require(remaining > 0, 'time_budget_exceeded')
        return min(30, remaining)


class DeadlineExpired(BaseException):
    """Never swallowed by metadata propagation retries or transport catches."""


@contextmanager
def network_budget(seconds=900):
    require(sys.platform=='linux' and threading.current_thread() is threading.main_thread(),
            'deadline_platform_unsupported')
    require(signal.getitimer(signal.ITIMER_REAL)==(0.0,0.0),'deadline_timer_in_use')
    old=signal.getsignal(signal.SIGALRM)
    def expired(_signum,_frame):raise DeadlineExpired()
    signal.signal(signal.SIGALRM,expired)
    try:
        signal.setitimer(signal.ITIMER_REAL,seconds)
        yield Deadline(seconds)
    finally:
        signal.setitimer(signal.ITIMER_REAL,0)
        signal.signal(signal.SIGALRM,old)


def bounded_metadata(response,limit,deadline):
    output=bytearray()
    while True:
        deadline.timeout()
        chunk=response.read1(min(65536,limit-len(output)+1))
        if not chunk:break
        output.extend(chunk)
        require(len(output)<=limit,'metadata_oversized')
    deadline.timeout()
    return bytes(output)


def allowed_url(url):
    parsed = urllib.parse.urlsplit(url)
    try: port = parsed.port
    except ValueError: raise Refused('redirect_refused')
    require(parsed.scheme == 'https' and parsed.hostname in HOSTS and port in (None, 443)
            and parsed.username is None and parsed.password is None and not parsed.fragment,
            'redirect_refused')
    return url


class Redirects(urllib.request.HTTPRedirectHandler):
    max_redirections = 5
    max_repeats = 2
    def redirect_request(self, request, response, code, message, headers, new_url):
        allowed_url(new_url)
        return super().redirect_request(request, response, code, message, headers, new_url)


class Public:
    def __init__(self, deadline):
        self.deadline = deadline
        # Never use HTTP(S)_PROXY, ~/.netrc, cookies, credentials, or GH_TOKEN.
        context=ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        context.load_default_certs()
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}),
            urllib.request.HTTPSHandler(context=context), Redirects())
    def open(self, url):
        allowed_url(url)
        response = self.opener.open(urllib.request.Request(url,
            headers={'User-Agent': 'NexusBro-public-verification', 'Accept-Encoding': 'identity'}),
            timeout=self.deadline.timeout())
        try:
            require(response.status == 200, 'public_http_failed')
            allowed_url(response.geturl())
            return response
        except BaseException:
            response.close();raise
    def metadata(self, url, limit):
        with self.open(url) as response:
            return bounded_metadata(response,limit,self.deadline)


def strict_json(raw):
    def unique(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, 'metadata_invalid')
            result[key] = value
        return result
    def invalid_constant(_):raise Refused('metadata_invalid')
    try: return json.loads(raw, object_pairs_hook=unique,parse_constant=invalid_constant)
    except (ValueError, UnicodeError): raise Refused('metadata_invalid')


def local(path, limit):
    require(path.is_file() and not path.is_symlink() and path.stat().st_size <= limit,
            'local_expectation_invalid')
    data = path.read_bytes()
    require(len(data) <= limit, 'local_expectation_invalid')
    return data


def decode64(value):
    try: return base64.b64decode(value, validate=True)
    except (ValueError, TypeError): raise Refused('signature_invalid')


def public_key(config):
    try:
        encoded = strict_json(local(config, META_LIMIT))['plugins']['updater']['pubkey']
        lines = decode64(encoded).decode('ascii').splitlines()
        require(len(lines) == 2, 'signature_invalid')
        packet = decode64(lines[1])
        require(len(packet) == 42 and packet[:2] == b'Ed', 'signature_invalid')
        return packet[2:10], Ed25519PublicKey.from_public_bytes(packet[10:])
    except (KeyError, UnicodeError, ValueError): raise Refused('signature_invalid')


def signature(encoded, key_id):
    try: lines = decode64(encoded).decode('utf-8').splitlines()
    except UnicodeError: raise Refused('signature_invalid')
    require(len(lines) == 4 and lines[0].startswith('untrusted comment: ')
            and lines[2].startswith('trusted comment: '), 'signature_invalid')
    packet, comment_sig = decode64(lines[1]), decode64(lines[3])
    require(len(packet) == 74 and packet[:2] == b'ED' and packet[2:10] == key_id
            and len(comment_sig) == 64, 'signature_invalid')
    return packet[10:], lines[2][len('trusted comment: '):].encode(), comment_sig


def verify_payload(response, expected, encoded_signature, key, deadline):
    size_expected = expected['bytes']
    require(type(size_expected) is int and 0 < size_expected <= PAYLOAD_LIMIT, 'payload_bound_invalid')
    declared = response.headers.get('Content-Length')
    require(isinstance(declared, str) and declared.isdecimal() and int(declared) == size_expected,
            'payload_length_mismatch')
    payload_sig, comment, comment_sig = signature(encoded_signature, key[0])
    sha, digest, size = hashlib.sha256(), hashlib.blake2b(), 0
    while True:
        deadline.timeout()
        chunk = response.read1(min(1024 * 1024, size_expected - size + 1))
        if not chunk: break
        size += len(chunk)
        require(size <= size_expected, 'payload_oversized')
        sha.update(chunk); digest.update(chunk)
    require(size == size_expected, 'payload_truncated')
    require(sha.hexdigest() == expected['sha256'], 'payload_sha_mismatch')
    try:
        key[1].verify(payload_sig, digest.digest())
        key[1].verify(comment_sig, payload_sig + comment)
    except Exception: raise Refused('signature_invalid')
    deadline.timeout()
    return {'file': expected['file'], 'bytes': size, 'sha256': sha.hexdigest(),
            'ed25519_payload_and_comment_verified': True, 'public_download_verified': True,
            'manifest_signature_matches':True}


def expectations(args):
    require(re.fullmatch(r'\d+\.\d+\.\d+', args.version) is not None
            and re.fullmatch(r'[0-9a-f]{40}', args.source) is not None
            and re.fullmatch(r'[1-9]\d{0,19}', args.build_run) is not None, 'arguments_invalid')
    manifest = strict_json(local(args.directory / 'latest.json', META_LIMIT))
    receipt = strict_json(local(args.directory / 'signature-verification.json', META_LIMIT))
    require(manifest.get('version') == receipt.get('version') == args.version, 'local_expectation_invalid')
    require(set(manifest.get('platforms', {})) == {*PLATFORMS.values(), 'linux-x86_64'}, 'local_expectation_invalid')
    rows = receipt.get('packages')
    require(isinstance(rows, list) and len(rows) == 3, 'local_expectation_invalid')
    expected = {}
    for row in rows:
        name = row.get('file') if isinstance(row, dict) else None
        require(isinstance(name, str) and re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._+-]{0,199}', name)
                and name not in expected and args.version in name, 'local_expectation_invalid')
        require(type(row.get('bytes')) is int and 0 < row['bytes'] <= PAYLOAD_LIMIT
                and re.fullmatch(r'[0-9a-f]{64}', row.get('sha256', '')) is not None,
                'local_expectation_invalid')
        expected[name] = row
    for suffix, platform in PLATFORMS.items():
        names = [name for name in expected if name.endswith(suffix)]
        require(len(names) == 1, 'local_expectation_invalid')
        name = names[0]; entry = manifest['platforms'][platform]
        require(entry['url'] == f'https://github.com/{REPO}/releases/download/v{args.version}/{name}', 'local_expectation_invalid')
        encoded = local(args.directory / (name + '.sig'), SIG_LIMIT).decode('ascii').strip()
        require(encoded == entry['signature'], 'local_expectation_invalid')
    require(manifest['platforms']['linux-x86_64'] == manifest['platforms']['linux-x86_64-deb'], 'local_expectation_invalid')
    return manifest, receipt, expected


def propagation_metadata(public, url, limit, expected, json_value):
    for attempt in range(3):
        try:
            raw = public.metadata(url, limit)
            observed = strict_json(raw) if json_value else raw.decode('ascii').strip()
            # Python equality equates True/1/1.0; the receipt comparison must
            # preserve JSON scalar types while ignoring whitespace/key ordering.
            matches=(json.dumps(observed,sort_keys=True,separators=(',',':'),allow_nan=False)==
                     json.dumps(expected,sort_keys=True,separators=(',',':'),allow_nan=False)) if json_value else observed==expected
            require(matches, 'public_metadata_mismatch')
            return
        except (Refused, OSError, UnicodeError):
            if attempt == 2: raise
            time.sleep(min(1 + attempt, public.deadline.timeout()))


def run(args, environment=None):
    proof = {'schema': 1, 'completed': False, 'repository': REPO, 'packages': []}
    try:
        admit(os.environ if environment is None else environment)  # BEFORE any request or opener.
        manifest, receipt, expected = expectations(args)
        proof.update(version=args.version, source=args.source, build_run=int(args.build_run))
        key = public_key(args.config)
        with network_budget() as deadline:
            public = Public(deadline)
            propagation_metadata(public, f'https://github.com/{REPO}/releases/latest/download/latest.json', META_LIMIT, manifest, True)
            propagation_metadata(public, f'https://github.com/{REPO}/releases/download/v{args.version}/signature-verification.json', META_LIMIT, receipt, True)
            for suffix, platform in PLATFORMS.items():
                entry = manifest['platforms'][platform]
                name = next(name for name in expected if name.endswith(suffix))
                propagation_metadata(public, entry['url'] + '.sig', SIG_LIMIT, entry['signature'], False)
                # Exactly one payload request/stream. Partial reads are never retried.
                with public.open(entry['url']) as response:
                    proof['packages'].append(verify_payload(response, expected[name], entry['signature'], key, public.deadline))
            require(len(proof['packages']) == 3, 'public_verification_incomplete')
            deadline.timeout()
            proof.update(completed=True, canonical_latest_verified=True, public_receipt_matches=True,
                         actual_public_downloads_verified=3,latest_endpoint_verified=True,
                         public_packages_downloaded_and_crypto_verified=True,published=True,
                         publication_receipt_matches=True,public_updater_verified=True)
    except DeadlineExpired: proof.update(completed=False,failure_code='time_budget_exceeded')
    except Refused as error: proof.update(completed=False,failure_code=error.code)
    except Exception: proof.update(completed=False,failure_code='verification_failed')
    if not proof['completed']:
        for name in ('canonical_latest_verified','public_receipt_matches','latest_endpoint_verified',
                     'public_packages_downloaded_and_crypto_verified','published','publication_receipt_matches','public_updater_verified'):
            proof.pop(name,None)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(proof, indent=2) + '\n')
    print(json.dumps({'completed': proof['completed'], 'failure_code': proof.get('failure_code')}))
    return 0 if proof['completed'] else 1


def self_test():
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
    from unittest import mock
    import tempfile
    from types import SimpleNamespace
    private = Ed25519PrivateKey.generate(); key_id = b'fixture!'; body = b'synthetic-public-payload'
    payload_sig = private.sign(hashlib.blake2b(body).digest()); comment = b'synthetic'
    packet = b'ED' + key_id + payload_sig
    encoded = base64.b64encode(('untrusted comment: fixture\n' + base64.b64encode(packet).decode()
        + '\ntrusted comment: synthetic\n' + base64.b64encode(private.sign(payload_sig + comment)).decode() + '\n').encode()).decode()
    row = {'file': 'Fixture_0.0.1.exe', 'bytes': len(body), 'sha256': hashlib.sha256(body).hexdigest()}
    def response(data):
        value = io.BytesIO(data); value.headers = {'Content-Length': str(len(body))}; return value
    assert verify_payload(response(body), row, encoded, (key_id, private.public_key()), Deadline())['bytes'] == len(body)
    failures = [(response(body), dict(row, sha256='0' * 64), encoded, 'payload_sha_mismatch'),
                (response(body[:-1]), row, encoded, 'payload_truncated'),
                (response(body + b'x'), row, encoded, 'payload_oversized'),
                (response(body), row, encoded, 'signature_invalid')]
    for index, (stream, expected, sig, code) in enumerate(failures):
        key = (key_id, Ed25519PrivateKey.generate().public_key()) if index == 3 else (key_id, private.public_key())
        try: verify_payload(stream, expected, sig, key, Deadline())
        except Refused as error: assert error.code == code
        else: raise AssertionError('Corruption accepted')
    for url in ('http://github.com/x', 'https://github.com.evil/x', 'https://u@github.com/x', 'https://github.com:444/x'):
        try: allowed_url(url)
        except Refused: pass
        else: raise AssertionError('Untrusted redirect accepted')
    try: strict_json(b'{"x":1,"x":2}')
    except Refused: pass
    else: raise AssertionError('Duplicate metadata accepted')
    class TypedMetadata:
        deadline=Deadline()
        def __init__(self,body):self.body=body;self.calls=0
        def metadata(self,*_):self.calls+=1;return self.body
    for body in (b'{"bytes":true}',b'{"bytes":1.0}',b'{"bytes":2}'):
        candidate=TypedMetadata(body)
        with mock.patch.object(time,'sleep'):
            try:propagation_metadata(candidate,'unused',META_LIMIT,{'bytes':1},True)
            except Refused as error:assert error.code=='public_metadata_mismatch'
            else:raise AssertionError('Typed metadata substitution accepted')
        assert candidate.calls==3
    try:strict_json(b'{"bytes":NaN}')
    except Refused:pass
    else:raise AssertionError('Nonfinite metadata accepted')
    try:verify_payload(response(body),dict(row,bytes=PAYLOAD_LIMIT+1),encoded,(key_id,private.public_key()),Deadline())
    except Refused as error:assert error.code=='payload_bound_invalid'
    else:raise AssertionError('Trusted payload bound ignored')
    try:bounded_metadata(io.BytesIO(b'x'*4),3,Deadline())
    except Refused as error:assert error.code=='metadata_oversized'
    else:raise AssertionError('Metadata bound ignored')
    class AlarmPublic:
        deadline=Deadline()
        def metadata(self,*_):raise DeadlineExpired()
    with mock.patch.object(time,'sleep',side_effect=AssertionError('Deadline retried')):
        try:propagation_metadata(AlarmPublic(),'unused',1,None,True)
        except DeadlineExpired:pass
        else:raise AssertionError('Global alarm swallowed')
    with mock.patch.object(signal,'getitimer',return_value=(1.0,0.0)),mock.patch.object(signal,'signal',side_effect=AssertionError('Existing timer overridden')):
        try:
            with network_budget():raise AssertionError('Active timer accepted')
        except Refused as error:assert error.code=='deadline_timer_in_use'
    with tempfile.TemporaryDirectory(prefix='nb-public-offline-') as temporary:
        args = SimpleNamespace(out=Path(temporary) / 'proof.json')
        with mock.patch.object(urllib.request, 'build_opener', side_effect=AssertionError('Network initialized')):
            assert run(args, {}) == 1
        proof = json.loads(args.out.read_text()); assert not proof['completed'] and proof['failure_code'] == 'not_github_actions'
    print(json.dumps({'self_test': True, 'public_network_requests': 0, 'product_acceptance': False}))


if __name__ == '__main__':
    if sys.argv[1:] == ['--self-test']: self_test(); raise SystemExit(0)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory', type=Path, required=True)
    parser.add_argument('--version', required=True)
    parser.add_argument('--source', required=True)
    parser.add_argument('--build-run', required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--config', type=Path, default=Path('desktop/src-tauri/tauri.conf.json'))
    raise SystemExit(run(parser.parse_args()))
