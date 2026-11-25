//! Native GTK title tooltips keep their contents and drawing but receive no
//! pointer input. Only GtkWindow/GdkWindow Tooltip hints are eligible.
//!
//! GtkWidget realize/map are RUN_FIRST without NO_HOOKS (GTK 3.24 gtkwidget.c).
//! GLib invokes emission hooks after the RUN_FIRST class closure: realize has
//! created the native window before we shape it; map reapplies the same policy.
//! https://docs.gtk.org/gobject/func.signal_add_emission_hook.html
//! https://docs.gtk.org/gdk3/method.Window.input_shape_combine_region.html
//! https://docs.gtk.org/gdk3/method.Window.set_pass_through.html

/// Call once from the application's GTK setup/UI thread. No effect elsewhere.
pub fn install() -> Result<(), &'static str> {
    #[cfg(target_os = "linux")]
    {
        linux::install()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use glib::translate::{from_glib_none, IntoGlib};
    use gtk::{gdk, glib, prelude::*};
    use std::{cell::Cell, os::raw::c_ulong};

    // Static hooks retain no widgets, callbacks, windows, titles, or user data.
    thread_local! {static HOOKS:Cell<Option<[c_ulong;2]>>=const {Cell::new(None)};}

    fn tooltip_input(value: *const glib::gobject_ffi::GValue) {
        // The map/realize instance parameter is a GtkWidget GValue, guaranteed
        // by the signal query at installation and this hook's ABI.
        let pointer = unsafe { glib::gobject_ffi::g_value_get_object(value) };
        if pointer.is_null() {
            return;
        }
        let object: glib::Object = unsafe { from_glib_none(pointer) };
        let Ok(window) = object.downcast::<gtk::Window>() else {
            return;
        };
        if window.type_hint() != gdk::WindowTypeHint::Tooltip {
            return;
        }
        let Some(native) = window.window() else {
            return;
        };
        if native.is_destroyed() || native.type_hint() != gdk::WindowTypeHint::Tooltip {
            return;
        }
        let empty = gtk::cairo::Region::create();
        if empty.status().is_err() || !empty.is_empty() {
            return;
        }
        // set_pass_through alone affects GDK dispatch, not native X11 input
        // shape. Empty INPUT shape makes the mapped tooltip transparent to
        // server-side pointer targeting, without changing its visual shape.
        native.set_pass_through(true);
        native.input_shape_combine_region(&empty, 0, 0);
    }

    unsafe extern "C" fn hook(
        _hint: *mut glib::gobject_ffi::GSignalInvocationHint,
        values_count: u32,
        values: *const glib::gobject_ffi::GValue,
        _data: glib::ffi::gpointer,
    ) -> glib::ffi::gboolean {
        if gtk::is_initialized_main_thread() && values_count == 1 && !values.is_null() {
            // Never unwind across the C signal boundary. Returning TRUE keeps
            // the hook installed; it does not stop or consume the GTK signal.
            let _ = std::panic::catch_unwind(|| tooltip_input(values));
        }
        true.into_glib()
    }

    pub(super) fn install() -> Result<(), &'static str> {
        if !gtk::is_initialized_main_thread() {
            return Err("gtk_tooltip_wrong_thread");
        }
        if HOOKS.with(|h| h.get().is_some()) {
            return Ok(());
        }
        // Initialize the inherited signal's class through the safe GLib API.
        let Some(_class) = glib::Class::<gtk::Widget>::from_type(gtk::Widget::static_type()) else {
            return Err("gtk_tooltip_class_unavailable");
        };
        let signals = ["realize", "map"]
            .map(|name| glib::subclass::signal::SignalId::lookup(name, gtk::Widget::static_type()));
        let [Some(realize), Some(map)] = signals else {
            return Err("gtk_tooltip_signals_unavailable");
        };
        for signal in [realize, map] {
            let query = signal.query();
            if query.type_() != gtk::Widget::static_type()
                || query.n_params() != 0
                || !query.flags().contains(glib::SignalFlags::RUN_FIRST)
                || query.flags().contains(glib::SignalFlags::NO_HOOKS)
            {
                return Err("gtk_tooltip_signal_contract_unsupported");
            }
        }
        let mut ids = [0; 2];
        for (index, signal) in [realize, map].into_iter().enumerate() {
            ids[index] = unsafe {
                // gtk-rs 0.18 has no safe emission-hook wrapper. The callback
                // is static and its one object parameter is checked above.
                glib::gobject_ffi::g_signal_add_emission_hook(
                    signal.into_glib(),
                    0,
                    Some(hook),
                    std::ptr::null_mut(),
                    None,
                )
            };
            if ids[index] == 0 {
                if index == 1 {
                    unsafe {
                        glib::gobject_ffi::g_signal_remove_emission_hook(
                            realize.into_glib(),
                            ids[0],
                        );
                    }
                }
                return Err("gtk_tooltip_hook_registration_failed");
            }
        }
        HOOKS.with(|h| h.set(Some(ids)));
        Ok(())
    }
}
