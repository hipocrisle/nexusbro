"""Canonical request targets for origin fixtures (never for proxy handlers).

An origin must accept both origin-form and absolute-form HTTP request targets.
Keep escaped path/query bytes intact: fixture receipts rely on their values.
"""
from urllib.parse import urlsplit


def canonicalize(target):
    if target.startswith('/'):
        # urlsplit treats a leading // as an authority, but it is a valid
        # origin-form path and must not lose its first component.
        return target.split('#', 1)[0]
    parsed = urlsplit(target)
    path = parsed.path or '/'
    has_query = '?' in target.split('#', 1)[0]
    return path + ('?' + parsed.query if has_query else '')


if __name__ == '__main__':
    import http.client
    import http.server
    import threading
    import unittest

    class OriginRequestTests(unittest.TestCase):
        def test_preserves_path_and_query(self):
            for target in ('/probe?case=before&value=%2F+x&empty=',
                           'http://127.0.0.1:1/probe?case=before&value=%2F+x&empty='):
                self.assertEqual(canonicalize(target), '/probe?case=before&value=%2F+x&empty=')
            self.assertEqual(canonicalize('//fixture/path?x=1'), '//fixture/path?x=1')
            self.assertEqual(canonicalize('http://127.0.0.1:1?'), '/?')

        def test_real_origin_dispatch_accepts_both_forms_and_post_body(self):
            class Handler(http.server.BaseHTTPRequestHandler):
                def log_message(self, *_):
                    pass

                def answer(self):
                    target = canonicalize(self.path)
                    if target != '/probe?case=before&empty=':
                        self.send_error(404)
                        return
                    size = int(self.headers.get('Content-Length', '0'))
                    body = self.rfile.read(size) if size else b''
                    response = target.encode('ascii') + b'\n' + body
                    self.send_response(200)
                    self.send_header('Content-Length', str(len(response)))
                    self.end_headers()
                    self.wfile.write(response)

                do_GET = answer
                do_POST = answer

            server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
            worker = threading.Thread(target=server.serve_forever, daemon=True)
            worker.start()
            try:
                relative = '/probe?case=before&empty='
                absolute = 'http://127.0.0.1:' + str(server.server_port) + relative
                for method, body in [('GET', None), ('POST', b'\x00\xfffixture-body')]:
                    responses = []
                    for target in (relative, absolute):
                        conn = http.client.HTTPConnection('127.0.0.1', server.server_port, timeout=2)
                        try:
                            conn.request(method, target, body=body)
                            response = conn.getresponse()
                            self.assertEqual(response.status, 200)
                            responses.append(response.read())
                        finally:
                            conn.close()
                    self.assertEqual(responses, [relative.encode() + b'\n' + (body or b'')] * 2)
            finally:
                server.shutdown()
                server.server_close()
                worker.join(timeout=2)

    unittest.main()
