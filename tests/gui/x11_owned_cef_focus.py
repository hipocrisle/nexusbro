#!/usr/bin/env python3
"""Owned-CEF X11 focus helper (test-only, Linux).

Selects the app-owned CEF browser window INSIDE the held shell XID by real
X11 facts — geometry (exact expected viewport ±2px), viewable map state and
the REAL creator PID via XResQueryClientIds (no _NET_WM_PID guessing, no
class-name guessing, GTK main excluded by construction: only descendants of
the held shell are considered) — then gives it REAL X input focus via
XSetInputFocus and proves XGetInputFocus afterwards. No JS focus/play, no
keyboard, no clicks, no root-window searches, no env/service changes.

The helper never trusts a PID by itself: the caller passes identity_check()
(held birth/exe verification from native process proofs), invoked before the
focus change and again after; any failure is a typed FocusBlocked, never a
fallback. The returned receipt carries no raw XIDs/PIDs/paths.
"""
import ctypes
import ctypes.util

XRES_CLIENTID_PID = 2
REVERT_TO_PARENT = 2
CURRENT_TIME = 0
MAX_CHILDREN = 64
MAX_DEPTH = 16
GEOMETRY_TOLERANCE = 2


class FocusBlocked(Exception):
    """Typed refusal; the message is a fixed code, never raw X11 data."""


# ---------------------------------------------------------------- pure part

def select_cef_candidate(nodes, viewport, allowed_pids, tolerance=GEOMETRY_TOLERANCE):
    """Pure selection over pre-collected node facts.

    nodes: iterable of dicts {xid, pid, x, y, w, h, viewable, depth, parent}.
    viewport: (x, y, w, h) in screen coordinates.
    allowed_pids: set of engine PIDs proved by the caller.
    Returns {'xid', 'depth', 'candidates'}; raises FocusBlocked otherwise."""
    vx, vy, vw, vh = viewport
    candidates = []
    for node in nodes:
        if not node.get('viewable'):
            continue
        if node.get('pid') not in allowed_pids:
            continue
        if (abs(node['x'] - vx) <= tolerance and abs(node['y'] - vy) <= tolerance
                and abs(node['w'] - vw) <= tolerance and abs(node['h'] - vh) <= tolerance):
            candidates.append(node)
    if not candidates:
        raise FocusBlocked('no_candidate')
    by_xid = {n['xid']: n for n in nodes}
    deepest = max(candidates, key=lambda n: n['depth'])

    def is_ancestor(maybe_ancestor, node):
        current = node
        for _ in range(MAX_DEPTH + 1):
            if current['xid'] == maybe_ancestor['xid']:
                return True
            parent = by_xid.get(current.get('parent'))
            if parent is None or parent['xid'] == current['xid']:
                return False
            current = parent
        return False

    # Один region может быть цепочкой вложенных окон одного размера — берём
    # самый глубокий лист. Независимые siblings в том же region — ambiguous.
    for other in candidates:
        if other['xid'] == deepest['xid']:
            continue
        if not is_ancestor(other, deepest):
            raise FocusBlocked('ambiguous_siblings')
    return {'xid': deepest['xid'], 'depth': deepest['depth'], 'candidates': len(candidates)}


# ------------------------------------------------------------- native layer

class _Attributes(ctypes.Structure):
    _fields_ = [('x', ctypes.c_int), ('y', ctypes.c_int), ('width', ctypes.c_int),
                ('height', ctypes.c_int), ('border_width', ctypes.c_int), ('depth', ctypes.c_int),
                ('visual', ctypes.c_void_p), ('root', ctypes.c_ulong), ('window_class', ctypes.c_int),
                ('bit_gravity', ctypes.c_int), ('win_gravity', ctypes.c_int), ('backing_store', ctypes.c_int),
                ('backing_planes', ctypes.c_ulong), ('backing_pixel', ctypes.c_ulong),
                ('save_under', ctypes.c_int), ('colormap', ctypes.c_ulong), ('map_installed', ctypes.c_int),
                ('map_state', ctypes.c_int), ('all_event_masks', ctypes.c_long),
                ('your_event_mask', ctypes.c_long), ('do_not_propagate_mask', ctypes.c_long),
                ('override_redirect', ctypes.c_int), ('screen', ctypes.c_void_p)]

IS_VIEWABLE = 2  # MapState: IsViewable


class _XErrorEvent(ctypes.Structure):
    _fields_ = [('type', ctypes.c_int), ('display', ctypes.c_void_p),
                ('resourceid', ctypes.c_ulong), ('serial', ctypes.c_ulong),
                ('error_code', ctypes.c_ubyte), ('request_code', ctypes.c_ubyte),
                ('minor_code', ctypes.c_ubyte)]


_XERRORHANDLER = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p, ctypes.POINTER(_XErrorEvent))


class _ClientIdSpec(ctypes.Structure):
    _fields_ = [('client', ctypes.c_ulong), ('mask', ctypes.c_uint)]


class _ClientIdValue(ctypes.Structure):
    _fields_ = [('spec', _ClientIdSpec), ('length', ctypes.c_long), ('value', ctypes.c_void_p)]


class NativeX11:
    """Real libX11/libXres binding; every symbol typed for 64-bit ABI.
    Missing library/extension is a typed refusal, never a fallback."""

    # ВНИМАНИЕ: XSetErrorHandler — process-global в libX11. Этот helper
    # рассчитан на owner-only single-thread использование (один owned display
    # в тестовом процессе); прежний handler сохраняется и восстанавливается
    # в close(). Callback-ссылка удерживается в self._error_handler_ref,
    # чтобы GC не собрал его, пока libX11 может его вызвать.
    def __init__(self, display_name):
        x11_path = ctypes.util.find_library('X11')
        xres_path = ctypes.util.find_library('XRes')
        if not x11_path:
            raise FocusBlocked('x11_library_missing')
        if not xres_path:
            raise FocusBlocked('xres_library_missing')
        self._bind(ctypes.CDLL(x11_path), ctypes.CDLL(xres_path), display_name)

    def _bind(self, x, r, display_name):
        Display = ctypes.c_void_p
        Window = ctypes.c_ulong
        x.XOpenDisplay.argtypes = [ctypes.c_char_p]; x.XOpenDisplay.restype = Display
        x.XCloseDisplay.argtypes = [Display]
        x.XQueryTree.argtypes = [Display, Window, ctypes.POINTER(Window), ctypes.POINTER(Window),
                                 ctypes.POINTER(ctypes.POINTER(Window)), ctypes.POINTER(ctypes.c_uint)]
        x.XQueryTree.restype = ctypes.c_int
        x.XFree.argtypes = [ctypes.c_void_p]
        x.XGetGeometry.argtypes = [Display, Window, ctypes.POINTER(Window), ctypes.POINTER(ctypes.c_int),
                                   ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_uint),
                                   ctypes.POINTER(ctypes.c_uint), ctypes.POINTER(ctypes.c_uint),
                                   ctypes.POINTER(ctypes.c_uint)]
        x.XGetGeometry.restype = ctypes.c_int
        x.XTranslateCoordinates.argtypes = [Display, Window, Window, ctypes.c_int, ctypes.c_int,
                                            ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_int),
                                            ctypes.POINTER(Window)]
        x.XTranslateCoordinates.restype = ctypes.c_int
        x.XGetWindowAttributes.argtypes = [Display, Window, ctypes.POINTER(_Attributes)]
        x.XGetWindowAttributes.restype = ctypes.c_int
        x.XSetInputFocus.argtypes = [Display, Window, ctypes.c_int, ctypes.c_ulong]
        x.XGetInputFocus.argtypes = [Display, ctypes.POINTER(Window), ctypes.POINTER(ctypes.c_int)]
        x.XSync.argtypes = [Display, ctypes.c_int]
        x.XSetErrorHandler.argtypes = [_XERRORHANDLER]
        x.XSetErrorHandler.restype = _XERRORHANDLER
        r.XResQueryVersion.argtypes = [Display, ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_int)]
        r.XResQueryVersion.restype = ctypes.c_int
        r.XResQueryClientIds.argtypes = [Display, ctypes.c_long, ctypes.POINTER(_ClientIdSpec),
                                         ctypes.POINTER(ctypes.c_long),
                                         ctypes.POINTER(ctypes.POINTER(_ClientIdValue))]
        r.XResQueryClientIds.restype = ctypes.c_int
        r.XResGetClientPid.argtypes = [ctypes.POINTER(_ClientIdValue)]
        r.XResGetClientPid.restype = ctypes.c_int
        # Официальный XRes.h: имя именно XResClientIdsDestroy.
        r.XResClientIdsDestroy.argtypes = [ctypes.c_long, ctypes.POINTER(_ClientIdValue)]
        self._x, self._r = x, r
        self.error_code = None
        self._previous_error_handler = None
        self._error_handler_ref = _XERRORHANDLER(self._on_x_error)
        self.display = x.XOpenDisplay(display_name.encode() if display_name else None)
        if not self.display:
            raise FocusBlocked('display_unavailable')
        self._previous_error_handler = x.XSetErrorHandler(self._error_handler_ref)
        major, minor = ctypes.c_int(), ctypes.c_int()
        # XRes.c: XResQueryVersion возвращает 1 при успехе (не 0).
        if r.XResQueryVersion(self.display, ctypes.byref(major), ctypes.byref(minor)) != 1 or major.value < 1:
            self.close()
            raise FocusBlocked('xres_extension_missing')

    def _on_x_error(self, _display, event):
        # Bounded: запоминаем только первый код (0..255); ничего не печатаем,
        # управление возвращаем — процесс не умирает мимо finally.
        if self.error_code is None and event:
            self.error_code = int(event.contents.error_code)
        return 0

    def _check_error(self):
        if self.error_code is not None:
            raise FocusBlocked('x_protocol_error')

    def close(self):
        if self.display:
            if self._previous_error_handler is not None:
                self._x.XSetErrorHandler(self._previous_error_handler)
                self._previous_error_handler = None
            self._x.XCloseDisplay(self.display)
            self.display = None

    def root(self):
        # DefaultRootWindow через XGetGeometry shell'а не нужен: root возьмём
        # из XQueryTree верхнего уровня вызывающего.
        return None

    def children(self, xid):
        x = self._x
        root = ctypes.c_ulong(); parent = ctypes.c_ulong()
        children = ctypes.POINTER(ctypes.c_ulong)(); count = ctypes.c_uint()
        if not x.XQueryTree(self.display, xid, ctypes.byref(root), ctypes.byref(parent),
                            ctypes.byref(children), ctypes.byref(count)):
            raise FocusBlocked('query_tree_failed')
        self._check_error()
        try:
            if count.value > MAX_CHILDREN:
                raise FocusBlocked('tree_bound_exceeded')
            return [children[i] for i in range(count.value)]
        finally:
            if children:
                x.XFree(children)

    def geometry(self, xid):
        x = self._x
        root = ctypes.c_ulong(); gx = ctypes.c_int(); gy = ctypes.c_int()
        w = ctypes.c_uint(); h = ctypes.c_uint(); border = ctypes.c_uint(); depth = ctypes.c_uint()
        if not x.XGetGeometry(self.display, xid, ctypes.byref(root), ctypes.byref(gx), ctypes.byref(gy),
                              ctypes.byref(w), ctypes.byref(h), ctypes.byref(border), ctypes.byref(depth)):
            raise FocusBlocked('geometry_failed')
        dx = ctypes.c_int(); dy = ctypes.c_int(); child = ctypes.c_ulong()
        # Экранные координаты: translate (0,0) окна в root; возврат проверяется.
        if not x.XTranslateCoordinates(self.display, xid, root.value, 0, 0,
                                       ctypes.byref(dx), ctypes.byref(dy), ctypes.byref(child)):
            raise FocusBlocked('translate_failed')
        self._check_error()
        attrs = _Attributes()
        if not x.XGetWindowAttributes(self.display, xid, ctypes.byref(attrs)):
            raise FocusBlocked('attributes_failed')
        self._check_error()
        return {'x': dx.value, 'y': dy.value, 'w': w.value, 'h': h.value,
                'viewable': attrs.map_state == IS_VIEWABLE}

    def pid_of(self, xid):
        r = self._r
        spec = _ClientIdSpec(client=xid, mask=XRES_CLIENTID_PID)
        count = ctypes.c_long()
        values = ctypes.POINTER(_ClientIdValue)()
        # У XResQueryClientIds успех == 0 (Status Success), см. XRes.h/XRes.c.
        status = r.XResQueryClientIds(self.display, 1, ctypes.byref(spec), ctypes.byref(count),
                                      ctypes.byref(values))
        if status != 0 or count.value < 1 or not values:
            raise FocusBlocked('client_pid_unavailable')
        try:
            return int(r.XResGetClientPid(ctypes.byref(values[0])))
        finally:
            r.XResClientIdsDestroy(count.value, values)

    def walk(self, shell_xid):
        """Bounded BFS внутри held shell: узлы с реальными фактами."""
        nodes = []
        frontier = [(shell_xid, 0, None)]
        seen = set()
        while frontier:
            xid, depth, parent = frontier.pop(0)
            if xid in seen or depth > MAX_DEPTH:
                continue
            seen.add(xid)
            if len(nodes) >= MAX_CHILDREN:
                raise FocusBlocked('tree_bound_exceeded')
            geo = self.geometry(xid)
            node = {'xid': xid, 'pid': self.pid_of(xid), 'depth': depth, 'parent': parent,
                    **geo}
            nodes.append(node)
            if depth < MAX_DEPTH:
                for child in self.children(xid):
                    frontier.append((child, depth + 1, xid))
        return nodes

    def set_focus(self, xid):
        self._x.XSetInputFocus(self.display, xid, REVERT_TO_PARENT, CURRENT_TIME)
        self._x.XSync(self.display, 0)
        self._check_error()

    def input_focus(self):
        x = self._x
        focus = ctypes.c_ulong(); revert = ctypes.c_int()
        x.XGetInputFocus(self.display, ctypes.byref(focus), ctypes.byref(revert))
        return focus.value


def _is_descendant(x11, ancestor, node):
    current = node
    for _ in range(MAX_DEPTH + 1):
        if current == ancestor:
            return True
        children_probe = None  # parent chain via walk facts would need nodes; use geometry-free query
        # Подъём по дереву через XQueryTree parent.
        import ctypes as c
        root = c.c_ulong(); parent = c.c_ulong()
        children = c.POINTER(c.c_ulong)(); count = c.c_uint()
        if not x11._x.XQueryTree(x11.display, current, c.byref(root), c.byref(parent),
                                 c.byref(children), c.byref(count)):
            return False
        if children:
            x11._x.XFree(children)
        if not parent.value or parent.value == current:
            return False
        current = parent.value
    return False


def _focus_flow(x11, shell_xid, viewport, engines, identity_check):
    allowed = set(engines)
    if not allowed:
        raise FocusBlocked('engines_empty')
    if identity_check() is not True:
        raise FocusBlocked('identity_unproven_before')
    nodes = x11.walk(shell_xid)
    selection = select_cef_candidate(nodes, viewport, allowed)
    selected = selection['xid']
    x11.set_focus(selected)
    focus = x11.input_focus()
    if focus != selected:
        # Допускается только строгий потомок выбранного окна того же engine.
        if focus <= 1 or focus not in {n['xid'] for n in x11.walk(shell_xid)
                                       if n['pid'] in allowed} or not _is_descendant(x11, selected, focus):
            raise FocusBlocked('focus_not_acquired')
    if x11.pid_of(selected) not in allowed:
        raise FocusBlocked('pid_changed_after_focus')
    if identity_check() is not True:
        raise FocusBlocked('identity_unproven_after')
    return {'focused': True, 'candidates': selection['candidates'],
            'candidate_depth': selection['depth'], 'geometry_match': True,
            'viewable': True, 'focus_exact': focus == selected,
            'pid_reverified': True, 'identity_before': True, 'identity_after': True}


def focus_cef(display, shell_xid, viewport, engines, identity_check):
    """API для ROOT: собственный display, held shell XID, ожидаемый CEF
    viewport в экранных координатах, разрешённые engine PID (из native
    process proofs) и identity_check() с held birth/exe сверкой.
    Возвращает безопасную квитанцию без сырых XID/PID/путей."""
    x11 = NativeX11(display)
    try:
        return _focus_flow(x11, shell_xid, viewport, engines, identity_check)
    finally:
        x11.close()


def inspect_tree(display, shell_xid):
    """Read-only снимок дерева held shell (для диагностики ROOT; наружу отчёты
    должны использовать санитизированные поля)."""
    x11 = NativeX11(display)
    try:
        return x11.walk(shell_xid)
    finally:
        x11.close()
