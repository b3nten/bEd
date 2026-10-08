#[cfg(target_os = "macos")]
pub mod macos_menu;
#[cfg(target_os = "macos")]
pub mod macos_window;

#[cfg(target_os = "linux")]
pub mod linux_window;
