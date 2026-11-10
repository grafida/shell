//! Native failure alerts (GTK). Used before the window exists (an invalid manifest) and when the
//! app cannot go on (the worker keeps crashing). stderr is not a user-facing channel for a desktop
//! app, so these always show a dialog; the same text also goes to the log. `SHELL_ALERTS=stderr`
//! keeps them to stderr (tests, headless smoke runs).

use gtk::prelude::*;
use tao::{platform::unix::WindowExtUnix, window::Window};

/// Shows an alert without a parent window, initialising GTK if needed. Falls back to stderr when
/// there is no display at all.
pub fn alert(title: &str, message: &str, detail: &str) {
    eprintln!("{title}: {message}\n{detail}");
    if std::env::var("SHELL_ALERTS").is_ok_and(|v| v == "stderr") || gtk::init().is_err() {
        return;
    }
    let dialog = build(None, title, message, detail);
    dialog.run();
    dialog.close();
}

/// The same alert over the app window, **without blocking**: a nested `run()` inside tao's event
/// handler would re-enter its loop. `on_close` runs when the user dismisses it.
pub fn show(
    parent: &Window,
    title: &str,
    message: &str,
    detail: &str,
    on_close: impl Fn() + 'static,
) {
    let dialog = build(Some(parent), title, message, detail);
    dialog.connect_response(move |d, _| {
        d.close();
        on_close();
    });
    dialog.show();
}

/// An error dialog with the detail (e.g. a stderr tail) in a scrollable monospace pane.
fn build(parent: Option<&Window>, title: &str, message: &str, detail: &str) -> gtk::MessageDialog {
    let gtk_parent = parent.map(|w| w.gtk_window().clone());
    let dialog = gtk::MessageDialog::new(
        gtk_parent.as_ref(),
        gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
        gtk::MessageType::Error,
        gtk::ButtonsType::Close,
        message,
    );
    dialog.set_title(title);

    if !detail.trim().is_empty() {
        let buffer = gtk::TextBuffer::new(None::<&gtk::TextTagTable>);
        buffer.set_text(detail);
        let view = gtk::TextView::with_buffer(&buffer);
        view.set_editable(false);
        view.set_monospace(true);
        view.set_wrap_mode(gtk::WrapMode::WordChar);
        let scroll = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        scroll.set_min_content_height(240);
        scroll.set_min_content_width(640);
        scroll.add(&view);
        dialog.content_area().pack_start(&scroll, true, true, 6);
        dialog.content_area().show_all();
    }

    dialog
}
