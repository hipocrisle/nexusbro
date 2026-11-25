/* NB_TOOLTIP_TRACE — read-only LD_PRELOAD pass-through observer (DIAGNOSTIC ONLY).
 *
 * Interposes exactly three GTK entry points of the UNCHANGED CI binary and
 * forwards every call with identical C ABI and arguments. Never writes GTK
 * state: no settings, titles, windows, shapes, raises, clicks, forced queries.
 *
 * Logging is active only when ALL hold:
 *   getenv("GITHUB_ACTIONS") == "true"
 *   getenv("NEXUSBRO_TEST_DATA") != NULL
 *   getenv("NEXUSBRO_TEST_TOOLTIP_TRACE_ARM") points to an EXISTING file
 *     (ROOT creates the marker just before ONE hover; the file's content is
 *     never read, existence is the arm signal).
 *
 * Output: at most 64 fixed-format lines per process, on stderr, each:
 *   NB_TOOLTIP_TRACE event=has|trigger|text requested=0|1 has=0|1 enabled=0|1 \
 *     timeout_ms=<0..120000> browse_ms=<0..120000> text_present=0|1
 * No paths, PIDs, titles, tooltip text, URLs, or raw dlerror strings.
 *
 * Widget filter: only instances whose type name starts with "WebKitWebView"
 * or contains a known WebKit prefix ("WebKit"). Unknown/other widgets are
 * forwarded but never logged. Pointers are passed through as-is; the hooked
 * functions receive the exact original valid widget pointers.
 */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define TRACE_PREFIX "NB_TOOLTIP_TRACE"
#define TRACE_MAX_LINES 64
#define TIMEOUT_CAP_MS 120000

typedef void *(*set_has_tooltip_fn)(void *widget, int state);
typedef void (*trigger_query_fn)(void *widget);
typedef void (*set_text_fn)(void *tooltip, const char *text);
typedef int (*get_has_tooltip_fn)(void *widget);
typedef void *(*get_settings_fn)(void *widget);
typedef const char *(*type_name_fn)(void *instance);
typedef void (*g_object_get_fn)(void *object, const char *first_property, ...);

static set_has_tooltip_fn real_set_has_tooltip;
static trigger_query_fn real_trigger_query;
static set_text_fn real_set_text;
static get_has_tooltip_fn real_get_has_tooltip;
static get_settings_fn real_get_settings;
static type_name_fn real_type_name;
static g_object_get_fn real_g_object_get;

static pthread_once_t resolve_once = PTHREAD_ONCE_INIT;
static atomic_int resolve_state; /* 0 = not ready, 1 = ok, 2 = hard failure */
static atomic_int armed = -1;    /* -1 unknown, 0 disarmed, 1 armed */
static atomic_uint line_count;
static atomic_int in_hook;

static void trace_resolve(void)
{
    real_set_has_tooltip = (set_has_tooltip_fn)dlsym(RTLD_NEXT, "gtk_widget_set_has_tooltip");
    real_trigger_query = (trigger_query_fn)dlsym(RTLD_NEXT, "gtk_widget_trigger_tooltip_query");
    real_set_text = (set_text_fn)dlsym(RTLD_NEXT, "gtk_tooltip_set_text");
    /* Optional read-only diagnostics; absence must not break forwarding. */
    real_get_has_tooltip = (get_has_tooltip_fn)dlsym(RTLD_NEXT, "gtk_widget_get_has_tooltip");
    real_get_settings = (get_settings_fn)dlsym(RTLD_NEXT, "gtk_widget_get_settings");
    real_type_name = (type_name_fn)dlsym(RTLD_DEFAULT, "g_type_name_from_instance");
    real_g_object_get = (g_object_get_fn)dlsym(RTLD_DEFAULT, "g_object_get");
    if (!real_set_has_tooltip || !real_trigger_query || !real_set_text) {
        atomic_store_explicit(&resolve_state, 2, memory_order_release);
        return;
    }
    atomic_store_explicit(&resolve_state, 1, memory_order_release);
}

static int trace_is_armed(void)
{
    int value = atomic_load_explicit(&armed, memory_order_acquire);
    const char *marker;
    if (value >= 0)
        return value;
    value = 0;
    if (getenv("GITHUB_ACTIONS") && strcmp(getenv("GITHUB_ACTIONS"), "true") == 0
        && getenv("NEXUSBRO_TEST_DATA")) {
        marker = getenv("NEXUSBRO_TEST_TOOLTIP_TRACE_ARM");
        if (marker && access(marker, F_OK) == 0)
            value = 1;
    }
    atomic_store_explicit(&armed, value, memory_order_release);
    return value;
}

static int trace_widget_known(void *widget)
{
    const char *name;
    if (!real_type_name || !widget)
        return 0;
    name = real_type_name(widget);
    if (!name)
        return 0;
    return strncmp(name, "WebKit", 6) == 0;
}

static int trace_line_left(void)
{
    if (atomic_load_explicit(&line_count, memory_order_relaxed) >= TRACE_MAX_LINES)
        return 0;
    atomic_fetch_add_explicit(&line_count, 1, memory_order_relaxed);
    return 1;
}

static void trace_emit(const char *event, void *widget, int requested, const char *text)
{
    int has = 0, enabled = 0, text_present = 0;
    long timeout_ms = 0, browse_ms = 0;
    void *settings;
    int guard;

    if (atomic_load_explicit(&in_hook, memory_order_acquire))
        return; /* recursion guard: never re-enter from our own diagnostics */
    guard = atomic_fetch_add_explicit(&in_hook, 1, memory_order_acq_rel);

    if (strcmp(event, "text") == 0)
        text_present = text && text[0] != '\0' ? 1 : 0;
    if (widget && real_get_has_tooltip)
        has = real_get_has_tooltip(widget) ? 1 : 0;
    if (widget && real_get_settings && real_g_object_get) {
        settings = real_get_settings(widget);
        if (settings)
            real_g_object_get(settings,
                              "gtk-enable-tooltips", &enabled,
                              "gtk-tooltip-timeout", &timeout_ms,
                              "gtk-tooltip-browse-timeout", &browse_ms,
                              NULL);
    }
    if (timeout_ms < 0 || timeout_ms > TIMEOUT_CAP_MS)
        timeout_ms = 0;
    if (browse_ms < 0 || browse_ms > TIMEOUT_CAP_MS)
        browse_ms = 0;

    fprintf(stderr, TRACE_PREFIX " event=%s requested=%d has=%d enabled=%d "
            "timeout_ms=%ld browse_ms=%ld text_present=%d\n",
            event, requested, has, enabled, timeout_ms, browse_ms, text_present);
    (void)guard;
    atomic_store_explicit(&in_hook, 0, memory_order_release);
}

void gtk_widget_set_has_tooltip(void *widget, int state)
{
    pthread_once(&resolve_once, trace_resolve);
    if (atomic_load_explicit(&resolve_state, memory_order_acquire) != 1)
        _exit(112); /* fixed numeric failure; no dlerror text */
    if (trace_is_armed() && trace_widget_known(widget) && trace_line_left())
        trace_emit("has", widget, state ? 1 : 0, NULL);
    real_set_has_tooltip(widget, state);
}

void gtk_widget_trigger_tooltip_query(void *widget)
{
    pthread_once(&resolve_once, trace_resolve);
    if (atomic_load_explicit(&resolve_state, memory_order_acquire) != 1)
        _exit(112);
    if (trace_is_armed() && trace_widget_known(widget) && trace_line_left())
        trace_emit("trigger", widget, 1, NULL);
    real_trigger_query(widget);
}

void gtk_tooltip_set_text(void *tooltip, const char *text)
{
    pthread_once(&resolve_once, trace_resolve);
    if (atomic_load_explicit(&resolve_state, memory_order_acquire) != 1)
        _exit(112);
    /* gtk_tooltip_set_text receives a tooltip, not a widget: no has/settings
     * reads are possible; only the text-present bit is logged. */
    if (trace_is_armed() && trace_line_left())
        trace_emit("text", NULL, 1, text);
    real_set_text(tooltip, text);
}
