#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    if std::env::args().any(|arg| arg == "--show-update") {
        unsafe {
            std::env::set_var("CODEXX_SHOW_UPDATE", "1");
        }
    }
    codexx_manager_lib::run();
}
