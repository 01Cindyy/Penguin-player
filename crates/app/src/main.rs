// On Windows, release builds are GUI programs, which open no console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() -> gtk::glib::ExitCode {
    prefer_vulkan_on_windows();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    penguin_player::run()
}

/// GTK on Windows draws with OpenGL unless told otherwise, which lags with some graphics
/// drivers that run Vulkan smoothly. A renderer the user chose with `GSK_RENDERER` is
/// kept, and where Vulkan doesn't start, GTK falls back to its usual choice.
fn prefer_vulkan_on_windows() {
    let chosen = std::env::var_os("GSK_RENDERER").filter(|value| !value.is_empty());
    if cfg!(windows) && chosen.is_none() {
        // SAFETY: nothing else runs yet, so no other thread reads the environment while
        // it changes.
        unsafe { std::env::set_var("GSK_RENDERER", "vulkan") };
    }
}
