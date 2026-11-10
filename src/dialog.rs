//! The native file dialogs behind `_native/dialog/*`, through rfd: the GTK portal on Linux,
//! `NSOpenPanel`/`NSSavePanel` on macOS. The platform layer queues a [`DialogRequest`] on the UI
//! thread; [`open`] builds the dialog there (parented to the window) and waits for it on a helper
//! thread, so the UI never blocks on a dialog.

use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, WindowHandle,
};
use tao::window::Window;

use crate::bridge::{DialogDone, FileFilter};

/// Dialog answers queued by the automation channel (`pick`); always empty without it.
pub type Picks = Arc<Mutex<VecDeque<Option<Vec<PathBuf>>>>>;

pub struct DialogRequest {
    pub kind: DialogKind,
    pub done: DialogDone,
}

impl std::fmt::Debug for DialogRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DialogRequest")
            .field("kind", &self.kind)
            .finish()
    }
}

#[derive(Debug)]
pub enum DialogKind {
    Open {
        filters: Vec<FileFilter>,
        multiple: bool,
    },
    Folder,
    Save {
        name: String,
        filters: Vec<FileFilter>,
    },
}

/// Answers from the automation channel's queue first, else opens the dialog. UI thread only.
pub fn answer(window: &Window, picks: &Picks, request: DialogRequest) {
    let queued = picks.lock().unwrap().pop_front();
    match queued {
        Some(answer) => (request.done)(answer),
        None => open(window, request),
    }
}

/// The window's raw handles, for parenting a dialog to it.
struct Parent(RawWindowHandle, RawDisplayHandle);

// SAFETY: the handles are plain identifiers copied from the live window on the UI thread; the
// dialog only uses them to name its parent while the window still exists.
unsafe impl Send for Parent {}
unsafe impl Sync for Parent {}

impl HasWindowHandle for Parent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        // SAFETY: see above.
        Ok(unsafe { WindowHandle::borrow_raw(self.0) })
    }
}

impl HasDisplayHandle for Parent {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        // SAFETY: see above.
        Ok(unsafe { DisplayHandle::borrow_raw(self.1) })
    }
}

/// Builds the dialog on the UI thread (parented to the window: a portal dialog on Linux, a sheet
/// on macOS) and waits for it on a helper thread.
pub fn open(window: &Window, request: DialogRequest) {
    let mut dialog = rfd::AsyncFileDialog::new();
    if let (Ok(w), Ok(d)) = (window.window_handle(), window.display_handle()) {
        dialog = dialog.set_parent(&Parent(w.as_raw(), d.as_raw()));
    }
    let add_filters = |mut dialog: rfd::AsyncFileDialog, filters: &[FileFilter]| {
        for f in filters {
            let ext: Vec<&str> = f.extensions.iter().map(String::as_str).collect();
            dialog = dialog.add_filter(&f.name, &ext);
        }
        dialog
    };
    let DialogRequest { kind, done } = request;
    let dialog = match &kind {
        DialogKind::Open { filters, .. } => add_filters(dialog, filters),
        DialogKind::Folder => dialog,
        DialogKind::Save { name, filters } => add_filters(dialog, filters).set_file_name(name),
    };

    std::thread::spawn(move || {
        let picked = match kind {
            DialogKind::Open {
                multiple: false, ..
            } => pollster::block_on(dialog.pick_file()).map(|f| vec![f.path().to_path_buf()]),
            DialogKind::Open { multiple: true, .. } => pollster::block_on(dialog.pick_files())
                .map(|fs| fs.iter().map(|f| f.path().to_path_buf()).collect()),
            DialogKind::Folder => {
                pollster::block_on(dialog.pick_folder()).map(|f| vec![f.path().to_path_buf()])
            }
            DialogKind::Save { .. } => {
                pollster::block_on(dialog.save_file()).map(|f| vec![f.path().to_path_buf()])
            }
        };
        done(picked);
    });
}
