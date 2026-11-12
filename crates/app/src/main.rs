// On Windows, release builds are GUI programs, which open no console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use mp_core::{AppPaths, Settings};

fn main() -> gtk::glib::ExitCode {
    limit_memory_pools();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    choose_renderer();
    penguin_player::run()
}

/// glibc gives threads that allocate at the same time memory pools of their own, which
/// each keep what was freed in them. Covers decode on many threads, so they share two.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn limit_memory_pools() {
    // SAFETY: only changes a setting of the allocator, before any other thread exists.
    unsafe { libc::mallopt(libc::M_ARENA_MAX, 2) };
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn limit_memory_pools() {}

/// Low Memory Mode draws with GTK's software renderer, which loads no graphics driver.
/// Otherwise GTK on Windows draws with OpenGL unless told otherwise, which lags with
/// some graphics drivers that run Vulkan smoothly. A renderer the user chose with
/// `GSK_RENDERER` is kept, and where Vulkan doesn't start, GTK falls back to its usual
/// choice.
fn choose_renderer() {
    let chosen = std::env::var_os("GSK_RENDERER").filter(|value| !value.is_empty());
    if chosen.is_some() {
        return;
    }
    let renderer = if low_memory() {
        "cairo"
    } else if cfg!(windows) {
        "vulkan"
    } else {
        return;
    };
    // SAFETY: nothing else runs yet, so no other thread reads the environment while it
    // changes.
    unsafe { std::env::set_var("GSK_RENDERER", renderer) };
}

/// Read before GTK starts, as GTK chooses its renderer once.
fn low_memory() -> bool {
    AppPaths::resolve().is_ok_and(|paths| Settings::load(&paths.settings_file()).low_memory)
}
