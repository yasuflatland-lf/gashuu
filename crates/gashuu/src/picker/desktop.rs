use super::{BookPicker, Done, PickedBook};

pub(super) struct DesktopPicker;

impl BookPicker for DesktopPicker {
    fn pick_books(&self, done: Done<Vec<PickedBook>>) {
        let dialog =
            rfd::FileDialog::new().add_filter("Comic archive", &["cbz", "zip", "cbr", "rar"]);
        #[cfg(target_os = "macos")]
        let paths = dialog.pick_files_or_folders();
        #[cfg(not(target_os = "macos"))]
        let paths = dialog.pick_files();
        let picks = paths
            .unwrap_or_default()
            .into_iter()
            .map(PickedBook::Path)
            .collect();
        let _ = slint::invoke_from_event_loop(move || done(picks));
    }

    fn supports_folders(&self) -> bool {
        true
    }

    fn pick_folder(&self, done: Done<Option<PickedBook>>) {
        let folder = rfd::FileDialog::new().pick_folder().map(PickedBook::Path);
        let _ = slint::invoke_from_event_loop(move || done(folder));
    }
}
