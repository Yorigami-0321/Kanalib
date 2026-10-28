use crate::*;

#[cfg(target_os = "windows")]
pub(crate) fn open_path(path: &PathBuf) {
    let _ = std::process::Command::new("explorer").arg(path).spawn();
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn open_path(path: &PathBuf) {
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}

#[cfg(target_os = "windows")]
pub(crate) fn reveal_path(path: &PathBuf) {
    let _ = std::process::Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .spawn();
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn reveal_path(path: &PathBuf) {
    if let Some(parent) = path.parent() {
        open_path(&parent.to_path_buf());
    }
}
