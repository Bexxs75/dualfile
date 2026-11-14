//! Builds the example plugin (`example/main.rs`) as a statically linked program for the
//! tests: once as the normal plugin and once with the `testbin` cfg (misbehaving modes, a
//! writable tree). It is compiled with `rustc` directly because `crt-static` cannot be set
//! for one binary of a workspace without breaking proc-macro crates.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    if std::env::var_os("CARGO_FEATURE_EXAMPLE_BINARIES").is_none() {
        return;
    }
    println!("cargo:rerun-if-changed=example/main.rs");
    println!("cargo:rerun-if-changed=build.rs");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    for (name, extra) in [
        ("example-plugin", None),
        ("example-plugin-testbin", Some("testbin")),
    ] {
        let target = out.join(name);
        let mut command = Command::new(&rustc);
        command
            .args([
                "--edition",
                "2021",
                "--crate-type",
                "bin",
                "-C",
                "opt-level=1",
            ])
            .args([
                "-C",
                "target-feature=+crt-static",
                "-C",
                "debuginfo=0",
                "-C",
                "panic=abort",
            ])
            .arg("-o")
            .arg(&target)
            .arg("example/main.rs");
        if let Some(cfg) = extra {
            command.args(["--cfg", cfg]);
        }
        let status = command.status().expect("rustc could not be started");
        assert!(status.success(), "building the example plugin failed");
        let key = name.to_uppercase().replace('-', "_");
        println!("cargo:rustc-env=DUALFILE_{key}={}", target.display());
    }
}
