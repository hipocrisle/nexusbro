/* NB_TOOLTIP_TRACE — read-only LD_PRELOAD pass-through observer (DIAGNOSTIC ONLY).
 *
 * Interposes exactly three GTK entry points of the UNCHANGED CI binary and
 * forwards every call with identical C ABI and arguments. Never writes GTK
 * state: no settings, titles, windows, shapes, raises, clicks, forced queries,
 * no synthetic tooltips, no show/hide/resize manipulation.
 *
 * Logging is active only when ALL hold:
 *   getenv("GITHUB_ACTIONS") == "true"
 *   getenv("NEXUSBRO_TEST_DATA") != NULL
 *   getenv("NEXUSBRO_TEST_TOOLTIP_TRACE_ARM") points to an EXISTING file
 *     (ROOT creates the marker just before ONE hover; the file's content is
 *     never read, existence is the arm signal).
 *
 * Output: at most 64 fixed-format lines per process, on stderr, each:
 *   NB_TOOLTIP_TRACE event=has|trigger|text|realize|map|unmap \
 *     observed=0|1 requested=0|1 has=0|1 enabled=0|1 \
 *     timeout_ms=<0..120000> browse_ms=<0..120000> text_present=0|1 \
 *     mapped=0|1 visible=0|1 hooks=0|1 tick_ms=<0..600000>
 * tick_ms is relative to the first successful symbol resolve. The hooks field
 * is 1 only when the optional GtkWidget realize/map/unmap emission hooks were
 * actually registered; hooks=0 is an EXPLICIT capability bit, never proof of
 * "no map happened".
 * No paths, PIDs, titles, tooltip text, URLs, or raw dlerror strings.
 *
 * Widget filter for has/trigger: only type names starting with "WebKit".
 * Signal records: only GtkWindow instances whose type hint is
 * GDK_WINDOW_TYPE_HINT_TOOLTIP (10) — no other widgets/windows are logged.
 */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#define TRACE_PREFIX "NB_TOOLTIP_TRACE"
#define TRACE_MAX_LINES 64
#define TIMEOUT_CAP_MS 120000
#define TICK_CAP_MS 600000
#define GDK_WINDOW_TYPE_HINT_TOOLTIP 10

typedef void (*set_has_tooltip_fn)(void *widget, int state);
typedef void (*trigger_query_fn)(void *widget);
typedef void (*set_text_fn)(void *tooltip, const char *text);
typedef int (*get_has_tooltip_fn)(void *widget);
typedef void *(*get_settings_fn)(void *widget);
typedef const char *(*type_name_fn)(void *instance);
typedef void (*g_object_get_fn)(void *object, const char *first_property, ...);
typedef int (*get_mapped_fn)(void *widget);
typedef int (*get_visible_fn)(void *widget);
typedef int (*emission_hook_fn)(void *, unsigned int, const void *, void *);
typedef void (*destroy_notify_fn)(void *);
typedef unsigned long (*signal_add_hook_fn)(unsigned int, unsigned int, emission_hook_fn, void *, destroy_notify_fn);
typedef unsigned int (*signal_lookup_fn)(const char *name, unsigned long type_id);
typedef void *(*value_get_object_fn)(const void *value);
typedef int (*type_check_instance_is_a_fn)(void *instance, unsigned long type);
typedef unsigned long (*gtk_window_get_type_fn)(void);
typedef int (*gtk_window_get_type_hint_fn)(void *window);
typedef unsigned long (*gtk_widget_get_type_fn)(void);

static set_has_tooltip_fn real_set_has_tooltip;
static trigger_query_fn real_trigger_query;
static set_text_fn real_set_text;
static get_has_tooltip_fn real_get_has_tooltip;
static get_settings_fn real_get_settings;
static type_name_fn real_type_name;
static g_object_get_fn real_g_object_get;
static get_mapped_fn real_get_mapped;
static get_visible_fn real_get_visible;
static signal_add_hook_fn real_signal_add_hook;
static signal_lookup_fn real_signal_lookup;
static value_get_object_fn real_value_get_object;
static type_check_instance_is_a_fn real_type_check;
static gtk_window_get_type_fn real_gtk_window_get_type;
static gtk_window_get_type_hint_fn real_gtk_window_get_type_hint;
static gtk_widget_get_type_fn real_gtk_widget_get_type;

static pthread_once_t resolve_once = PTHREAD_ONCE_INIT;
static pthread_once_t hooks_once = PTHREAD_ONCE_INIT;
static atomic_int resolve_state; /* 0 = not ready, 1 = ok, 2 = hard failure */
static atomic_int hooks_state = 0; /* 0 = not registered, 1 = registered */
static atomic_uint line_count;
static struct timespec tick_origin;
static _Thread_local int in_hook;

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
    real_get_mapped = (get_mapped_fn)dlsym(RTLD_NEXT, "gtk_widget_get_mapped");
    real_get_visible = (get_visible_fn)dlsym(RTLD_NEXT, "gtk_widget_get_visible");
    real_signal_add_hook = (signal_add_hook_fn)dlsym(RTLD_DEFAULT, "g_signal_add_emission_hook");
    real_signal_lookup = (signal_lookup_fn)dlsym(RTLD_DEFAULT, "g_signal_lookup");
    real_value_get_object = (value_get_object_fn)dlsym(RTLD_DEFAULT, "g_value_get_object");
    real_type_check = (type_check_instance_is_a_fn)dlsym(RTLD_DEFAULT, "g_type_check_instance_is_a");
    real_gtk_window_get_type = (gtk_window_get_type_fn)dlsym(RTLD_DEFAULT, "gtk_window_get_type");
    real_gtk_window_get_type_hint = (gtk_window_get_type_hint_fn)dlsym(RTLD_NEXT, "gtk_window_get_type_hint");
    real_gtk_widget_get_type = (gtk_widget_get_type_fn)dlsym(RTLD_DEFAULT, "gtk_widget_get_type");
    clock_gettime(CLOCK_MONOTONIC, &tick_origin);
    if (!real_set_has_tooltip || !real_trigger_query || !real_set_text) {
        atomic_store_explicit(&resolve_state, 2, memory_order_release);
        return;
    }
    atomic_store_explicit(&resolve_state, 1, memory_order_release);
}

static int trace_is_armed(void)
{
    const char *gha = getenv("GITHUB_ACTIONS");
    const char *data = getenv("NEXUSBRO_TEST_DATA");
    const char *marker = getenv("NEXUSBRO_TEST_TOOLTIP_TRACE_ARM");
    return gha && strcmp(gha, "true") == 0 && data && data[0]
        && marker && access(marker, F_OK) == 0;
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
    return atomic_fetch_add_explicit(&line_count, 1, memory_order_relaxed) < TRACE_MAX_LINES;
}

static unsigned int trace_tick_ms(void)
{
    struct timespec now;
    long long ms;
    clock_gettime(CLOCK_MONOTONIC, &now);
    ms = (long long)(now.tv_sec - tick_origin.tv_sec) * 1000
       + (now.tv_nsec - tick_origin.tv_nsec) / 1000000;
    if (ms < 0)
        return 0;
    if (ms > TICK_CAP_MS)
        return TICK_CAP_MS;
    return (unsigned int)ms;
}

static void trace_emit_line(const char *event, int observed, int requested, int has,
                            int enabled, unsigned int timeout_ms, unsigned int browse_ms,
                            int text_present, int mapped, int visible)
{
    fprintf(stderr, TRACE_PREFIX " event=%s observed=%d requested=%d has=%d enabled=%d "
            "timeout_ms=%u browse_ms=%u text_present=%d mapped=%d visible=%d hooks=%d tick_ms=%u\n",
            event, observed, requested, has, enabled ? 1 : 0, timeout_ms, browse_ms,
            text_present, mapped, visible,
            atomic_load_explicit(&hooks_state, memory_order_acquire), trace_tick_ms());
}

static void trace_emit(const char *event, void *widget, int requested, const char *text)
{
    int has = 0, enabled = 0, text_present = 0, observed = 0, mapped = 0, visible = 0;
    unsigned int timeout_ms = 0, browse_ms = 0;
    void *settings;

    if (in_hook)
        return; /* recursion guard: never re-enter from our own diagnostics */
    in_hook = 1;

    if (strcmp(event, "text") == 0)
        text_present = text && text[0] != '\0' ? 1 : 0;
    if (widget && real_get_has_tooltip)
        has = real_get_has_tooltip(widget) ? 1 : 0;
    if (widget && real_get_mapped)
        mapped = real_get_mapped(widget) ? 1 : 0;
    if (widget && real_get_visible)
        visible = real_get_visible(widget) ? 1 : 0;
    if (widget && real_get_settings && real_g_object_get) {
        settings = real_get_settings(widget);
        if (settings) {
            observed = real_get_has_tooltip != NULL;
            real_g_object_get(settings,
                              "gtk-enable-tooltips", &enabled,
                              "gtk-tooltip-timeout", &timeout_ms,
                              "gtk-tooltip-browse-timeout", &browse_ms,
                              NULL);
        }
    }
    if (timeout_ms > TIMEOUT_CAP_MS)
        timeout_ms = 0;
    if (browse_ms > TIMEOUT_CAP_MS)
        browse_ms = 0;

    trace_emit_line(event, observed, requested, has, enabled, timeout_ms, browse_ms, text_present, mapped, visible);
    in_hook = 0;
}

/* GtkWidget realize/map/unmap emission-hook callback. Exact GLib ABI expressed
 * without GLib headers. Returns TRUE to keep the hook installed; never consumes
 * or rewrites any signal value. No object references are retained and no
 * pointer outlives this callback. */
static int trace_signal_cb(void *hint, unsigned int nparams, const void *raw_values, void *data)
{
    const char *event = (const char *)data;
    void *object;
    int mapped = 0, visible = 0;

    (void)hint;
    if (nparams != 1 || !raw_values || !real_value_get_object || !trace_is_armed())
        return 1;
    if (in_hook)
        return 1;
    in_hook = 1;
    object = real_value_get_object(raw_values); /* First GValue itself, not its g_type as a pointer. */
    if (object && real_type_check && real_gtk_window_get_type && real_gtk_window_get_type_hint) {
        unsigned long window_type = real_gtk_window_get_type();
        if (window_type && real_type_check(object, window_type)
            && real_gtk_window_get_type_hint(object) == GDK_WINDOW_TYPE_HINT_TOOLTIP) {
            if (real_get_mapped)
                mapped = real_get_mapped(object) ? 1 : 0;
            if (real_get_visible)
                visible = real_get_visible(object) ? 1 : 0;
            if (trace_is_armed() && trace_line_left())
                trace_emit_line(event, 0, 1, 0, 0, 0, 0, 0, mapped, visible);
        }
    }
    in_hook = 0;
    return 1;
}

static void trace_register_hooks(void)
{
    static const char *const names[3] = {"realize", "map", "unmap"};
    int index;
    unsigned int signal_id;
    int registered = 0;

    if (in_hook)
        return;
    in_hook = 1;
    if (real_signal_add_hook && real_signal_lookup && real_gtk_widget_get_type
        && real_value_get_object && real_type_check
        && real_gtk_window_get_type && real_gtk_window_get_type_hint
        && real_get_mapped && real_get_visible) {
        unsigned long widget_type = real_gtk_widget_get_type();
        if (widget_type) {
            for (index = 0; index < 3; index++) {
                signal_id = real_signal_lookup(names[index], widget_type);
                if (signal_id && real_signal_add_hook(signal_id, 0, trace_signal_cb, (void *)names[index], NULL))
                    registered++;
            }
            atomic_store_explicit(&hooks_state, registered == 3, memory_order_release);
        }
    }
    in_hook = 0;
}

static void trace_ensure_hooks(void)
{
    if (!in_hook)
        pthread_once(&hooks_once, trace_register_hooks);
}

void gtk_widget_set_has_tooltip(void *widget, int state)
{
    pthread_once(&resolve_once, trace_resolve);
    if (atomic_load_explicit(&resolve_state, memory_order_acquire) != 1)
        _exit(112); /* fixed numeric failure; no dlerror text */
    trace_ensure_hooks();
    if (trace_is_armed() && trace_widget_known(widget) && trace_line_left())
        trace_emit("has", widget, state ? 1 : 0, NULL);
    real_set_has_tooltip(widget, state);
}

void gtk_widget_trigger_tooltip_query(void *widget)
{
    pthread_once(&resolve_once, trace_resolve);
    if (atomic_load_explicit(&resolve_state, memory_order_acquire) != 1)
        _exit(112);
    trace_ensure_hooks();
    if (trace_is_armed() && trace_widget_known(widget) && trace_line_left())
        trace_emit("trigger", widget, 1, NULL);
    real_trigger_query(widget);
}

void gtk_tooltip_set_text(void *tooltip, const char *text)
{
    pthread_once(&resolve_once, trace_resolve);
    if (atomic_load_explicit(&resolve_state, memory_order_acquire) != 1)
        _exit(112);
    trace_ensure_hooks();
    /* gtk_tooltip_set_text receives a tooltip, not a widget: no has/settings
     * reads are possible; only the text-present bit is logged. */
    if (trace_is_armed() && trace_line_left())
        trace_emit("text", NULL, 1, text);
    real_set_text(tooltip, text);
}
