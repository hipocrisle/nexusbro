#!/usr/bin/env python3
"""Offline tests for x11_owned_cef_focus (no X server, no GUI).

Pure selection negatives (foreign PID, geometry mismatch, ambiguous siblings,
invisible) and the full _focus_flow against a fake X11 object: identity gate
before/after, focus acquire, descendant-focus acceptance, pid-change refusal.
"""
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'tests' / 'gui'))
import x11_owned_cef_focus as m


def node(xid, pid, x, y, w, h, viewable=True, depth=1, parent=None):
    return {'xid': xid, 'pid': pid, 'x': x, 'y': y, 'w': w, 'h': h,
            'viewable': viewable, 'depth': depth, 'parent': parent}


VIEW = (180, 95, 1100, 725)
ENGINE = {777}


def expect_blocked(fn, code):
    try:
        fn()
    except m.FocusBlocked as error:
        return str(error) == code
    return False


def main():
    checks = {}

    # --- pure selection ---
    chain = [node(1, 111, 0, 0, 1280, 900, depth=0, parent=None),
             node(2, 777, 180, 95, 1100, 725, depth=1, parent=1),
             node(3, 777, 180, 95, 1100, 725, depth=2, parent=2)]
    picked = m.select_cef_candidate(chain, VIEW, ENGINE)
    checks['deepest_leaf_selected'] = picked['xid'] == 3 and picked['candidates'] == 2
    checks['geometry_mismatch'] = expect_blocked(
        lambda: m.select_cef_candidate([node(2, 777, 180, 95, 900, 700)], VIEW, ENGINE), 'no_candidate')
    checks['tolerance_edge_ok'] = m.select_cef_candidate(
        [node(2, 777, 182, 93, 1098, 727)], VIEW, ENGINE)['xid'] == 2
    checks['tolerance_edge_fail'] = expect_blocked(
        lambda: m.select_cef_candidate([node(2, 777, 183, 95, 1100, 725)], VIEW, ENGINE), 'no_candidate')
    checks['foreign_pid_rejected'] = expect_blocked(
        lambda: m.select_cef_candidate([node(2, 555, 180, 95, 1100, 725)], VIEW, ENGINE), 'no_candidate')
    checks['invisible_rejected'] = expect_blocked(
        lambda: m.select_cef_candidate([node(2, 777, 180, 95, 1100, 725, viewable=False)], VIEW, ENGINE),
        'no_candidate')
    siblings = [node(1, 111, 0, 0, 1280, 900, depth=0, parent=None),
                node(2, 777, 180, 95, 1100, 725, depth=1, parent=1),
                node(3, 777, 180, 95, 1100, 725, depth=1, parent=1)]
    checks['ambiguous_siblings'] = expect_blocked(
        lambda: m.select_cef_candidate(siblings, VIEW, ENGINE), 'ambiguous_siblings')

    # --- flow with fake X11 ---
    class FakeX11:
        def __init__(self, nodes, focus_after=None, pid_changes=False):
            self.nodes = nodes
            self.focus = 0
            self.focus_after = focus_after
            self.pid_changes = pid_changes
            self._x = None
        def walk(self, shell):
            return list(self.nodes)
        def set_focus(self, xid):
            self.focus = self.focus_after if self.focus_after is not None else xid
        def input_focus(self):
            return self.focus
        def pid_of(self, xid):
            if self.pid_changes:
                return 555
            return next(n['pid'] for n in self.nodes if n['xid'] == xid)

    identity_calls = []
    def identity_ok():
        identity_calls.append(1)
        return True

    good = FakeX11(chain)
    receipt = m._focus_flow(good, 1, VIEW, ENGINE, identity_ok)
    checks['flow_success'] = receipt['focused'] is True and receipt['focus_exact'] is True \
        and receipt['candidate_depth'] == 2
    checks['receipt_no_raw_values'] = all(isinstance(v, (bool, int)) for v in receipt.values()) \
        and 'xid' not in json.dumps(receipt) and '777' not in json.dumps(receipt)
    checks['identity_called_twice'] = len(identity_calls) == 2

    checks['identity_before_blocks'] = expect_blocked(
        lambda: m._focus_flow(FakeX11(chain), 1, VIEW, ENGINE, lambda: False), 'identity_unproven_before')
    calls = []
    def identity_after_false():
        calls.append(1)
        return len(calls) < 2
    checks['identity_after_blocks'] = expect_blocked(
        lambda: m._focus_flow(FakeX11(chain), 1, VIEW, ENGINE, identity_after_false),
        'identity_unproven_after')
    checks['pid_change_blocks'] = expect_blocked(
        lambda: m._focus_flow(FakeX11(chain, pid_changes=True), 1, VIEW, ENGINE, identity_ok),
        'pid_changed_after_focus')
    checks['focus_not_acquired'] = expect_blocked(
        lambda: m._focus_flow(FakeX11(chain, focus_after=999), 1, VIEW, ENGINE, identity_ok),
        'focus_not_acquired')
    checks['engines_empty_blocks'] = expect_blocked(
        lambda: m._focus_flow(FakeX11(chain), 1, VIEW, set(), identity_ok), 'engines_empty')

    # descendant-focus допускается только внутри того же engine: выбран
    # deepest кандидат 3, X ставит фокус на его потомка 4 (не кандидат по
    # геометрии, но тот же engine и строго внутри дерева).
    deep = chain + [node(4, 777, 180, 95, 800, 600, depth=3, parent=3)]
    fake = FakeX11(deep, focus_after=4)
    receipt = None
    try:
        original = m._is_descendant
        m._is_descendant = lambda x11, ancestor, node: (ancestor, node) == (3, 4)
        receipt = m._focus_flow(fake, 1, VIEW, ENGINE, identity_ok)
    finally:
        m._is_descendant = original
    checks['descendant_focus_accepted'] = receipt['focused'] is True \
        and receipt['focus_exact'] is False and receipt['candidate_depth'] == 2

    # исходники: никаких JS/кликов/клавиатуры
    src = (Path(__file__).resolve().parents[1] / 'tests' / 'gui' / 'x11_owned_cef_focus.py').read_text()
    checks['no_js_no_click_no_keys'] = all(token not in src for token in
        ('anchor.click', 'xdotool click', 'pyautogui', "key'", 'XTestFakeKey', 'XTestFakeButton'))
    checks['receipt_keys_fixed'] = set(receipt) == {'focused', 'candidates', 'candidate_depth',
        'geometry_match', 'viewable', 'focus_exact', 'pid_reverified', 'identity_before', 'identity_after'}

    # --- fake-CDLL ABI регрессии: обе конвенции возвратов и cleanup ---
    class FakeFn:
        def __init__(self, impl):
            self.impl = impl
            self.argtypes = None
            self.restype = None
        def __call__(self, *args):
            return self.impl(*args)

    class FakeLib:
        def __init__(self, **funcs):
            for name, impl in funcs.items():
                setattr(self, name, FakeFn(impl))

    def fire_error(handler, code):
        event = m._XErrorEvent()
        event.error_code = code
        handler(None, ctypes.byref(event))

    import ctypes
    def make_bare(version_ret=1, pid_status=0, translate_ret=1, query_fail=False):
        inst = m.NativeX11.__new__(m.NativeX11)
        state = {'closed': 0, 'destroyed': 0, 'handler': None}
        buf_holder = {}
        def open_display(name):
            return 0xD15
        def close_display(d):
            state['closed'] += 1
        def set_handler(h):
            old = state['handler']
            state['handler'] = h
            return old if old is not None else m._XERRORHANDLER(lambda d, e: 0)
        def query_version(d, major, minor):
            major._obj.value = 1; minor._obj.value = 2
            return version_ret
        def query_client_ids(d, n, specs, count, values):
            if pid_status != 0:
                return pid_status
            value = m._ClientIdValue()
            value.length = 1
            value.value = None
            arr = (m._ClientIdValue * 1)(value)
            count._obj.value = 1
            out = ctypes.cast(values, ctypes.POINTER(ctypes.c_void_p))
            out.contents.value = ctypes.addressof(arr)
            buf_holder['arr'] = arr
            return 0
        def get_pid(value_ptr):
            return 777
        def destroy(n, values):
            state['destroyed'] += 1
        def translate(d, src, dst, sx, sy, dx, dy, child):
            dx._obj.value = 180; dy._obj.value = 95
            return translate_ret
        x = FakeLib(XOpenDisplay=open_display, XCloseDisplay=close_display,
                    XSetErrorHandler=set_handler,
                    XQueryTree=lambda *a: 1, XFree=lambda p: None,
                    XGetGeometry=lambda *a: 1, XTranslateCoordinates=translate,
                    XGetWindowAttributes=lambda *a: 1, XSetInputFocus=lambda *a: None,
                    XGetInputFocus=lambda *a: None, XSync=lambda *a: None)
        r = FakeLib(XResQueryVersion=query_version, XResQueryClientIds=query_client_ids,
                    XResGetClientPid=get_pid, XResClientIdsDestroy=destroy)
        inst._bind(x, r, ':99')
        return inst, state

    bare, state = make_bare()
    checks['abi_bind_version_success_is_one'] = bare.display == 0xD15
    checks['abi_pid_success_is_zero'] = bare.pid_of(0xA) == 777 and state['destroyed'] == 1
    try:
        make_bare(version_ret=0)
        checks['abi_version_zero_blocked'] = False
    except m.FocusBlocked as error:
        checks['abi_version_zero_blocked'] = str(error) == 'xres_extension_missing'
    bare2, state2 = make_bare()
    # cleanup при version fail: display закрыт внутри _bind
    try:
        inst = m.NativeX11.__new__(m.NativeX11)
        inst._bind(FakeLib(XOpenDisplay=lambda n: 0xD16, XCloseDisplay=lambda d: state2.__setitem__('closed', state2['closed'] + 1),
                           XSetErrorHandler=lambda h: m._XERRORHANDLER(lambda dd, e: 0),
                           XQueryTree=lambda *a: 1, XFree=lambda p: None, XGetGeometry=lambda *a: 1,
                           XTranslateCoordinates=lambda *a: 1, XGetWindowAttributes=lambda *a: 1,
                           XSetInputFocus=lambda *a: None, XGetInputFocus=lambda *a: None,
                           XSync=lambda *a: None),
                   FakeLib(XResQueryVersion=lambda d, ma, mi: 0,
                           XResQueryClientIds=lambda *a: 0, XResGetClientPid=lambda v: 1,
                           XResClientIdsDestroy=lambda *a: None), ':99')
        checks['abi_version_fail_cleanup'] = False
    except m.FocusBlocked:
        checks['abi_version_fail_cleanup'] = state2['closed'] == 1
    # XTranslateCoordinates возврат проверяется
    bare3, _ = make_bare(translate_ret=0)
    try:
        bare3.geometry(0xA)
        checks['translate_zero_blocked'] = False
    except m.FocusBlocked as error:
        checks['translate_zero_blocked'] = str(error) == 'translate_failed'
    # X error handler: BadWindow из меняющегося дерева -> typed, не смерть процесса
    bare4, state4 = make_bare()
    fire_error(state4['handler'], 3)  # BadWindow
    try:
        bare4._check_error()
        checks['x_error_typed'] = False
    except m.FocusBlocked as error:
        checks['x_error_typed'] = str(error) == 'x_protocol_error'
    checks['x_error_bounded_first_only'] = bare4.error_code == 3
    fire_error(state4['handler'], 9)
    checks['x_error_first_kept'] = bare4.error_code == 3
    # close() восстанавливает прежний handler
    bare4.close()
    checks['close_restores_handler'] = bare4.display is None

    failed = [name for name, ok in checks.items() if not ok]
    print(json.dumps({'x11_focus_checks': 'PASS' if not failed else 'FAIL', 'failed': failed}))
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main())
