// Prevents an extra console window on Windows in release; no effect on Linux.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(unix)]
    if dualfile_remote::askpass::helper_requested() {
        std::process::exit(dualfile_remote::askpass::run_askpass_helper(
            std::env::args_os(),
            std::env::vars_os(),
        ));
    }
    dualfile_lib::run()
}
