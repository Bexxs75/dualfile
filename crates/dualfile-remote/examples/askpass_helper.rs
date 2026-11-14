//! Minimal embedding executable: the real application uses the same early entry point.
fn main() {
    #[cfg(unix)]
    if dualfile_remote::askpass::helper_requested() {
        std::process::exit(dualfile_remote::askpass::run_askpass_helper(
            std::env::args_os(),
            std::env::vars_os(),
        ));
    }
}
