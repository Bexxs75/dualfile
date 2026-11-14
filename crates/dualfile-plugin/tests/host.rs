//! The plugin process: environment, limits, sandbox, protocol violations, clean-up.

mod common;
use common::*;
use dualfile_plugin::{
    protocol, ErrorCode, Kind, PinnedState, PluginClient, PluginError, PluginRuntime, Session,
};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

type Check = fn(&PluginError) -> bool;

fn misbehave(client: &PluginClient, mode: &str) -> Result<Value, PluginError> {
    client.raw_call("debug_misbehave", json!({ "mode": mode }))
}

fn outcome(client: &PluginClient, mode: &str) -> (String, String) {
    let value = misbehave(client, mode).unwrap();
    (
        value["outcome"].as_str().unwrap().to_owned(),
        value["detail"].as_str().unwrap().to_owned(),
    )
}

fn proc_exists(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

fn proc_text(pid: u32, file: &str) -> String {
    std::fs::read_to_string(format!("/proc/{pid}/{file}")).unwrap_or_default()
}

/// Gone, or at most a zombie of somebody else's child (never one of ours).
fn wait_gone(pid: u32) -> bool {
    for _ in 0..100 {
        if !proc_exists(pid)
            || proc_text(pid, "stat")
                .split(") ")
                .nth(1)
                .is_some_and(|s| s.starts_with('Z'))
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

fn denied(detail: &str) -> bool {
    detail.contains("Operation not permitted") || detail.contains("Permission denied")
}

// ---------------------------------------------------------------- environment and process

#[test]
fn the_plugin_gets_an_empty_environment_with_a_fixed_list_and_fixed_arguments() {
    // the host process carries variables a plugin must never see
    std::env::set_var("SSH_AUTH_SOCK", "/tmp/agent.sock");
    std::env::set_var("DBUS_SESSION_BUS_ADDRESS", "unix:path=/tmp/bus");
    std::env::set_var("DUALFILE_TEST_SECRET_TOKEN", "tok");
    std::env::set_var("LD_LIBRARY_PATH", "/nonexistent-dualfile-test");
    std::env::set_var("AWS_SECRET_ACCESS_KEY", "key");
    let env = Env::new();
    let client = env.client(&Spec::testbin("envtest"));
    let value = misbehave(&client, "env").unwrap();
    let vars: Vec<(String, String)> = value["vars"]
        .as_array()
        .unwrap()
        .iter()
        .map(|pair| {
            (
                pair[0].as_str().unwrap().to_owned(),
                pair[1].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let names: Vec<&str> = vars.iter().map(|(k, _)| k.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(sorted, ["HOME", "LANG", "PATH", "TMPDIR"], "{vars:?}");
    let get = |key: &str| vars.iter().find(|(k, _)| k == key).unwrap().1.clone();
    assert_eq!(get("LANG"), "C.UTF-8");
    assert_eq!(get("PATH"), "/nonexistent");
    let work = get("HOME");
    assert_eq!(get("TMPDIR"), work);
    assert_eq!(value["cwd"], work);
    assert!(Path::new(&work).starts_with(std::env::temp_dir()) && Path::new(&work).exists());
    assert!(!vars.iter().any(|(_, v)| v.contains("agent.sock")
        || v.contains("tok")
        || v.contains("nonexistent-dualfile")));
    // the arguments are the fixed protocol identification, the program path is absolute
    let args: Vec<&str> = value["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert_eq!(args.len(), 2);
    assert!(Path::new(args[0]).is_absolute() && args[0].ends_with("/envtest/example-plugin"));
    assert_eq!(args[1], "--dualfile-plugin-api=1");
    // the working folder is empty and deleted with the session
    assert_eq!(std::fs::read_dir(&work).unwrap().count(), 0);
    client.shutdown();
    assert!(!Path::new(&work).exists());
}

#[test]
fn the_process_runs_in_its_own_group_with_limits_and_no_new_privileges() {
    let env = Env::new();
    let client = env.client(&Spec::testbin("limits"));
    client.fs_list("/").unwrap();
    let pid = client.pid().unwrap();
    let stat = proc_text(pid, "stat");
    let fields: Vec<&str> = stat.rsplit_once(") ").unwrap().1.split(' ').collect();
    assert_eq!(
        fields[2].parse::<u32>().unwrap(),
        pid,
        "own process group (leader)"
    );
    let limits = proc_text(pid, "limits");
    let row = |name: &str| {
        limits
            .lines()
            .find(|l| l.starts_with(name))
            .unwrap_or_default()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    assert!(row("Max cpu time").contains("600 600"), "{limits}");
    assert!(row("Max core file size").contains(" 0 0"), "{limits}");
    assert!(row("Max file size").contains(" 0 0"), "{limits}");
    assert!(row("Max processes").contains(" 0 0"), "{limits}");
    assert!(row("Max open files").contains(" 32 32"), "{limits}");
    assert!(
        row("Max address space").contains("1073741824 1073741824"),
        "{limits}"
    );
    let status = proc_text(pid, "status");
    assert!(status.contains("NoNewPrivs:\t1"), "{status}");
    assert!(
        status.contains("Seccomp:\t2"),
        "seccomp filter active: {status}"
    );
    // only the three standard descriptors survive the start
    let fds = std::fs::read_dir(format!("/proc/{pid}/fd"))
        .unwrap()
        .count();
    assert!(fds <= 4, "{fds} descriptors open");
    assert_eq!(client.is_sandboxed(), Some(true));
}

// ---------------------------------------------------------------- sandbox

#[test]
fn the_sandbox_is_available_on_this_machine() {
    let info =
        dualfile_plugin::sandbox::probe().expect("Landlock and seccomp are needed for these tests");
    assert!(info.landlock_abi >= 1);
    eprintln!("sandbox: {info:?}");
}

#[test]
fn the_sandbox_stops_network_files_and_programs() {
    let env = Env::new();
    let client = env.client(&Spec::testbin("sandbox"));
    for mode in ["net", "udp", "unix_connect"] {
        let (result, detail) = outcome(&client, mode);
        assert_eq!(result, "denied", "{mode}");
        assert!(denied(&detail), "{mode}: {detail}");
    }
    // nothing is written, inside the plugin folder or outside it
    let escape = format!("/tmp/dualfile-plugin-escape-{}", client.pid().unwrap());
    for mode in ["write_tmp", "write_plugin_dir", "mkdir_plugin_dir"] {
        let (result, detail) = outcome(&client, mode);
        assert_eq!(result, "denied", "{mode}");
        assert!(denied(&detail), "{mode}: {detail}");
    }
    assert!(!Path::new(&escape).exists());
    assert!(
        !env.base().join("sandbox/escape.txt").exists()
            && !env.base().join("sandbox/escape-dir").exists()
    );
    // reading and listing outside the plugin folder fails (the file is readable for the user,
    // so this is the sandbox, not the file permissions)
    assert!(std::fs::read("/etc/passwd").is_ok());
    for mode in ["read_outside", "list_outside"] {
        let (result, detail) = outcome(&client, mode);
        assert_eq!(result, "denied", "{mode}");
        assert!(denied(&detail), "{mode}: {detail}");
    }
    // its own folder is readable
    assert_eq!(outcome(&client, "read_own").0, "allowed");
    // no shell, no other program, no new process
    for mode in ["shell", "shell_own_dir", "thread"] {
        let (result, detail) = outcome(&client, mode);
        assert_eq!(result, "denied", "{mode}: {detail}");
    }
    // the listed dangerous system calls are all refused, a harmless one works
    let calls = misbehave(&client, "syscalls").unwrap();
    for name in [
        "ptrace",
        "mount",
        "unshare",
        "setns",
        "bpf",
        "kexec_load",
        "init_module",
        "socket_inet",
        "socket_netlink",
        "socket_packet",
        "socket_unix",
        "execve",
        "fork",
        "kill_init",
        "memfd_create",
        "io_uring_setup",
    ] {
        let ret = calls[name].as_i64().unwrap();
        assert!(ret < 0, "{name} returned {ret}");
    }
    assert!(calls["getpid"].as_i64().unwrap() > 0);
    // the session itself still works after all those refusals
    assert!(!client.fs_list("/").unwrap().is_empty());
}

#[test]
fn without_the_explicit_flag_the_plugin_never_runs_unsandboxed() {
    let env = Env::new();
    let spec = Spec::testbin("flag");
    install(env.base(), &spec);
    let package = dualfile_plugin::verify_package(env.base(), "flag").unwrap();
    let pinned = PinnedState::for_package(&package, false);
    let runtime = env.runtime();
    let mut session = Session::start(&runtime, &package, &pinned).unwrap();
    assert!(session.is_sandboxed());
    session.kill();
    assert!(env.log.lock().unwrap().is_empty());
}

#[test]
fn unsandboxed_needs_the_explicit_flag_in_the_pinned_state_and_is_logged() {
    let env = Env::new();
    let spec = Spec::testbin("nosandbox");
    install(env.base(), &spec);
    let package = dualfile_plugin::verify_package(env.base(), "nosandbox").unwrap();
    let runtime = env.runtime();
    let pinned = PinnedState::for_package(&package, true);
    assert!(pinned.unsandboxed);
    let client = PluginClient::open(runtime, pinned).unwrap();
    client.fs_list("/").unwrap();
    assert_eq!(client.is_sandboxed(), Some(false));
    let log = env.log.lock().unwrap().clone();
    assert!(
        log.iter()
            .any(|l| l.contains("nosandbox") && l.contains("ohne Sandbox")),
        "{log:?}"
    );
    // without the sandbox the file rules do not apply (the file may be read), the network
    // is merely refused by the other side; the resource limits and no_new_privs still do
    assert_eq!(outcome(&client, "read_outside").0, "allowed");
    let (result, detail) = outcome(&client, "net");
    assert_eq!(result, "denied");
    assert!(!denied(&detail), "not stopped by a sandbox: {detail}");
    let pid = client.pid().unwrap();
    let status = proc_text(pid, "status");
    assert!(
        status.contains("NoNewPrivs:\t1") && status.contains("Seccomp:\t0"),
        "{status}"
    );
    // the flag is part of the pinned state: a different value is a different state
    let other = PinnedState::for_package(&package, false);
    assert_ne!(other, PinnedState::for_package(&package, true));
}

#[test]
fn when_the_sandbox_cannot_be_built_the_plugin_does_not_start() {
    let env = Env::new();
    let spec = Spec::testbin("nosb");
    install(env.base(), &spec);
    let mut config = env.config();
    config.simulate_sandbox_unavailable = true;
    let runtime = PluginRuntime::new(config);
    let package = dualfile_plugin::verify_package(env.base(), "nosb").unwrap();
    let pinned = PinnedState::for_package(&package, false);
    assert!(matches!(
        Session::start(&runtime, &package, &pinned),
        Err(PluginError::SandboxUnavailable)
    ));
    assert_eq!(runtime.live_processes(), 0);
    let client = PluginClient::open(runtime.clone(), pinned).unwrap();
    assert!(matches!(
        client.fs_list("/"),
        Err(PluginError::SandboxUnavailable)
    ));
    assert!(client.pid().is_none());
    // a configuration problem is not a plugin failure: nothing counts towards deactivation
    for _ in 0..5 {
        assert!(matches!(
            client.fs_list("/"),
            Err(PluginError::SandboxUnavailable)
        ));
    }
    assert!(!client.health().disabled);
    // the user's explicit choice still works (and is logged)
    let loose = PinnedState::for_package(&package, true);
    let client = PluginClient::open(runtime, loose).unwrap();
    client.fs_list("/").unwrap();
}

// ---------------------------------------------------------------- violations and clean-up

#[test]
fn a_time_out_kills_and_reaps_the_process() {
    let env = Env::new();
    let client = env.client(&Spec::testbin("slow"));
    client.fs_list("/").unwrap();
    for mode in ["sleep", "loop"] {
        let pid = client.pid().unwrap();
        let started = Instant::now();
        let error = client
            .raw_call_with_timeout(
                "debug_misbehave",
                json!({ "mode": mode }),
                Duration::from_millis(400),
            )
            .unwrap_err();
        assert!(matches!(error, PluginError::Timeout), "{mode}: {error:?}");
        assert!(started.elapsed() < Duration::from_secs(3), "{mode}");
        // killed and reaped: no process and no zombie is left behind
        assert!(!proc_exists(pid), "{mode}: process {pid} still listed");
        // the plugin recovers with a fresh process
        assert!(!client.fs_list("/").unwrap().is_empty());
        assert_ne!(client.pid(), Some(pid));
    }
}

#[test]
fn crashes_exits_and_broken_answers_are_errors_and_never_zombies() {
    let env = Env::new();
    let mut config = env.config();
    config.max_failures = 100;
    let spec = Spec::testbin("broken");
    install(env.base(), &spec);
    let client = env.client_for(&spec, PluginRuntime::new(config));
    let cases: [(&str, Check); 8] = [
        ("crash", |e| matches!(e, PluginError::Crashed)),
        ("exit", |e| matches!(e, PluginError::Crashed)),
        ("huge", |e| matches!(e, PluginError::Protocol(_))),
        ("huge_line", |e| matches!(e, PluginError::Protocol(_))),
        ("badjson", |e| matches!(e, PluginError::Protocol(_))),
        ("unsolicited", |e| matches!(e, PluginError::Protocol(_))),
        ("wrong_id", |e| matches!(e, PluginError::Protocol(_))),
        ("bad_schema", |e| matches!(e, PluginError::Protocol(_))),
    ];
    for (mode, check) in cases {
        let _ = client.fs_list("/").unwrap();
        let pid = client.pid().unwrap();
        let started = Instant::now();
        let error = misbehave(&client, mode).unwrap_err();
        assert!(check(&error), "{mode}: {error:?}");
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "{mode} took {:?}",
            started.elapsed()
        );
        assert!(!proc_exists(pid), "{mode}: process {pid} still listed");
        // plugin text is never in the message
        assert!(!error.to_string().contains("aaaa"));
    }
    client.shutdown();
}

#[test]
fn unknown_fields_in_answers_and_markup_previews_are_refused() {
    let env = Env::new();
    let client = env.client(&Spec::testbin("preview"));
    let result = client.raw_call("debug_misbehave", json!({"mode": "bad_schema"}));
    assert!(matches!(result, Err(PluginError::Protocol(_))));
    // the test double answers with a markup preview; the typed check refuses it
    let markup = client
        .raw_call("debug_misbehave", json!({"mode": "html_preview"}))
        .unwrap();
    assert!(protocol::parse_preview(&markup).is_err());
}

#[test]
fn a_standard_error_flood_is_capped_and_shown_as_clean_text() {
    let env = Env::new();
    let client = env.client(&Spec::testbin("flood"));
    let started = Instant::now();
    misbehave(&client, "stderr_flood").unwrap();
    assert!(started.elapsed() < Duration::from_secs(4));
    let text = client.diagnostic();
    assert!(
        text.len() <= dualfile_plugin::error::MAX_DIAGNOSTIC_BYTES,
        "{}",
        text.len()
    );
    assert!(!text.chars().any(char::is_control));
    assert!(!text.is_empty());
    // the plugin is still usable
    assert!(!client.fs_list("/").unwrap().is_empty());
}

#[test]
fn first_exchange_problems_stop_the_start() {
    for (mode, check) in [
        (
            "api2",
            (|e: &PluginError| matches!(e, PluginError::Protocol(_))) as fn(&PluginError) -> bool,
        ),
        ("garbage", |e| matches!(e, PluginError::Protocol(_))),
        ("exit_at_start", |e| matches!(e, PluginError::Crashed)),
        ("silent", |e| matches!(e, PluginError::Timeout)),
    ] {
        let env = Env::new();
        let spec = Spec::testbin("hello").mode(mode);
        install(env.base(), &spec);
        let mut config = env.config();
        config.hello_timeout = Duration::from_millis(600);
        let runtime = PluginRuntime::new(config);
        let package = dualfile_plugin::verify_package(env.base(), "hello").unwrap();
        let pinned = PinnedState::for_package(&package, false);
        let started = Instant::now();
        let error = Session::start(&runtime, &package, &pinned)
            .err()
            .unwrap_or_else(|| panic!("{mode} must fail"));
        assert!(check(&error), "{mode}: {error:?}");
        assert!(started.elapsed() < Duration::from_secs(4), "{mode}");
        assert_eq!(runtime.live_processes(), 0, "{mode}");
    }
}

#[test]
fn a_plugin_may_not_claim_kinds_the_manifest_does_not_declare() {
    let env = Env::new();
    let mut spec = Spec::testbin("kinds");
    spec.kinds = vec!["filesystem"]; // the program says it also does packer, column, preview
    install(env.base(), &spec);
    let package = dualfile_plugin::verify_package(env.base(), "kinds").unwrap();
    let pinned = PinnedState::for_package(&package, false);
    let runtime = env.runtime();
    let error = Session::start(&runtime, &package, &pinned).err().unwrap();
    assert!(matches!(error, PluginError::Protocol(_)), "{error:?}");
    assert_eq!(runtime.live_processes(), 0);
}

#[test]
fn a_crash_loop_switches_the_plugin_off_until_the_user_resets_it() {
    let env = Env::new();
    let client = env.client(&Spec::testbin("loop"));
    for round in 1..=3 {
        assert!(
            matches!(misbehave(&client, "crash"), Err(PluginError::Crashed)),
            "round {round}"
        );
        assert_eq!(client.health().failures, round);
    }
    assert!(client.health().disabled);
    // no process is started any more
    assert!(matches!(client.fs_list("/"), Err(PluginError::Disabled)));
    assert!(client.pid().is_none());
    assert!(env
        .log
        .lock()
        .unwrap()
        .iter()
        .any(|l| l.contains("deaktiviert")));
    // the user switches it on again
    client.reset_health();
    assert!(!client.fs_list("/").unwrap().is_empty());
    assert_eq!(client.health().failures, 0);
}

#[test]
fn a_success_between_failures_resets_the_count() {
    let env = Env::new();
    let client = env.client(&Spec::testbin("reset"));
    for _ in 0..5 {
        assert!(misbehave(&client, "crash").is_err());
        assert!(!client.fs_list("/").unwrap().is_empty());
        assert_eq!(client.health().failures, 0);
    }
    assert!(!client.health().disabled);
}

#[test]
fn after_a_failure_the_restart_waits_for_the_backoff() {
    let env = Env::new();
    let mut config = env.config();
    config.backoff = Duration::from_millis(500);
    let spec = Spec::testbin("backoff");
    install(env.base(), &spec);
    let client = env.client_for(&spec, PluginRuntime::new(config));
    assert!(matches!(
        misbehave(&client, "crash"),
        Err(PluginError::Crashed)
    ));
    // too soon: refused without starting anything
    assert!(matches!(client.fs_list("/"), Err(PluginError::Backoff)));
    assert!(client.pid().is_none());
    assert!(!client.health().disabled);
    std::thread::sleep(Duration::from_millis(600));
    assert!(!client.fs_list("/").unwrap().is_empty());
}

#[test]
fn a_process_that_died_while_idle_is_replaced() {
    let env = Env::new();
    let client = env.client(&Spec::testbin("idle"));
    client.fs_list("/").unwrap();
    let old = client.pid().unwrap();
    // SAFETY: killing the plugin process of this test.
    unsafe { libc_kill(old as i32) };
    std::thread::sleep(Duration::from_millis(100));
    assert!(!client.fs_list("/").unwrap().is_empty());
    assert_ne!(client.pid(), Some(old));
    assert!(!proc_exists(old));
}

extern "C" {
    #[link_name = "kill"]
    fn libc_kill_raw(pid: i32, sig: i32) -> i32;
}

unsafe fn libc_kill(pid: i32) {
    libc_kill_raw(pid, 9);
}

#[test]
fn the_number_of_running_plugin_processes_is_capped() {
    let env = Env::new();
    let mut config = env.config();
    config.max_processes = 2;
    let runtime = PluginRuntime::new(config);
    let clients: Vec<Arc<PluginClient>> = ["cap-a", "cap-b", "cap-c"]
        .iter()
        .map(|id| {
            let spec = Spec::testbin(id);
            install(env.base(), &spec);
            env.client_for(&spec, runtime.clone())
        })
        .collect();
    clients[0].fs_list("/").unwrap();
    clients[1].fs_list("/").unwrap();
    assert_eq!(runtime.live_processes(), 2);
    assert!(matches!(
        clients[2].fs_list("/"),
        Err(PluginError::TooManyProcesses)
    ));
    // running out of slots is no fault of the plugin
    assert_eq!(clients[2].health().failures, 0);
    clients[0].shutdown();
    assert_eq!(runtime.live_processes(), 1);
    clients[2].fs_list("/").unwrap();
    drop(clients);
    assert_eq!(runtime.live_processes(), 0);
}

#[test]
fn dropping_the_client_ends_and_reaps_the_process() {
    let env = Env::new();
    let client = env.client(&Spec::testbin("dropped"));
    client.fs_list("/").unwrap();
    let pid = client.pid().unwrap();
    drop(client);
    assert!(!proc_exists(pid));
}

#[test]
fn the_plugin_dies_with_the_program() {
    // a helper copy of this test binary starts a plugin and waits; it is killed hard and the
    // plugin must follow (parent-death signal)
    if std::env::var_os("DUALFILE_TEST_HELPER").is_some() {
        return;
    }
    let env = Env::new();
    let spec = Spec::testbin("orphan");
    install(env.base(), &spec);
    let exe = std::env::current_exe().unwrap();
    let mut helper = std::process::Command::new(exe)
        .args([
            "--exact",
            "helper_starts_a_plugin_and_waits",
            "--nocapture",
            "--include-ignored",
        ])
        .env("DUALFILE_TEST_HELPER", env.base())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = std::io::BufReader::new(helper.stdout.take().unwrap());
    let mut pid = None;
    let mut line = String::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        line.clear();
        if std::io::BufRead::read_line(&mut reader, &mut line).unwrap() == 0 {
            break;
        }
        if let Some(text) = line.strip_prefix("PLUGIN-PID ") {
            pid = text.trim().parse::<u32>().ok();
            break;
        }
    }
    let pid = pid.expect("the helper started the plugin");
    assert!(proc_exists(pid));
    helper.kill().unwrap();
    helper.wait().unwrap();
    assert!(
        wait_gone(pid),
        "plugin {pid} survived the death of the program"
    );
}

#[test]
#[ignore = "only run by the_plugin_dies_with_the_program"]
fn helper_starts_a_plugin_and_waits() {
    let Some(base) = std::env::var_os("DUALFILE_TEST_HELPER") else {
        return;
    };
    let mut config = dualfile_plugin::RuntimeConfig::new(&base);
    config.log = Some(Arc::new(|_| {}));
    let runtime = PluginRuntime::new(config);
    let package = dualfile_plugin::verify_package(Path::new(&base), "orphan").unwrap();
    let client = PluginClient::open(runtime, PinnedState::for_package(&package, false)).unwrap();
    client.fs_list("/").unwrap();
    println!("PLUGIN-PID {}", client.pid().unwrap());
    std::thread::sleep(Duration::from_secs(120));
}

// ---------------------------------------------------------------- typed services

#[test]
fn columns_previews_and_packers_work_and_are_declared_per_kind() {
    let env = Env::new();
    let client = env.client(&Spec::normal("services"));
    let items = vec![
        dualfile_plugin::ColumnItem {
            name: "abc.txt".into(),
            size: Some(3),
            modified: None,
        },
        dualfile_plugin::ColumnItem {
            name: "ä".into(),
            size: None,
            modified: Some(std::time::UNIX_EPOCH),
        },
    ];
    assert_eq!(
        client.column_values("len", &items).unwrap(),
        [Some("7".into()), Some("1".into())]
    );
    assert_eq!(
        client.column_values("upper", &items).unwrap()[0].as_deref(),
        Some("ABC.TXT")
    );
    assert!(matches!(
        client.column_values("nope", &items),
        Err(PluginError::Unsupported(_))
    ));
    let many = vec![items[0].clone(); protocol::MAX_COLUMN_ITEMS + 1];
    assert!(client.column_values("len", &many).is_err());
    assert_eq!(
        client
            .preview_render("a.txt", b"line one\nline two", 50)
            .unwrap(),
        protocol::Preview::Text("Vorschau: line one".into())
    );
    assert!(matches!(
        client.preview_render("a.png", b"x", 50).unwrap(),
        protocol::Preview::Image {
            mime: "image/png",
            ..
        }
    ));
    assert!(client
        .preview_render("a.txt", &vec![0u8; protocol::MAX_PREVIEW_INPUT + 1], 5)
        .is_err());
    let archive = b"dir/a.txt\tAAA\nb.txt\tBB\n";
    let list = client.packer_list("x.xyz", archive).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].size, 3);
    assert!(list.iter().all(|e| e.problem.is_none()));
    assert_eq!(
        client.packer_read("x.xyz", archive, "b.txt").unwrap(),
        b"BB"
    );
    assert!(matches!(
        client.packer_read("x.xyz", archive, "missing"),
        Err(PluginError::Reported {
            code: ErrorCode::NotFound,
            ..
        })
    ));
    // a plugin error answer is not a failure of the plugin
    assert_eq!(client.health().failures, 0);
    // kinds that the manifest does not declare are refused before anything is sent
    let mut spec = Spec::normal("fsonly");
    spec.kinds = vec!["filesystem"];
    let env2 = Env::new();
    install(env2.base(), &spec);
    let package = dualfile_plugin::verify_package(env2.base(), "fsonly").unwrap();
    let narrow = PluginClient::open(env2.runtime(), PinnedState::for_package(&package, false));
    // (the program announces more kinds than declared, so it does not even start)
    assert!(narrow.unwrap().column_values("len", &items).is_err());
    let _ = Kind::Column;
}

#[test]
fn oversized_requests_are_refused_without_blaming_the_plugin() {
    let env = Env::new();
    let mut config = env.config();
    config.max_request_bytes = 2000;
    let spec = Spec::normal("bigreq");
    install(env.base(), &spec);
    let client = env.client_for(&spec, PluginRuntime::new(config));
    assert!(matches!(
        client.preview_render("a.txt", &[b'x'; 4000], 5),
        Err(PluginError::Unsupported(_))
    ));
    assert_eq!(client.health().failures, 0);
    assert!(client.preview_render("a.txt", b"small", 5).is_ok());
}

#[test]
fn a_plugin_started_by_a_short_lived_thread_keeps_running() {
    // the parent-death signal follows the thread that forked; the host starts plugins from
    // a thread of its own so that job threads may come and go
    let env = Env::new();
    let client = env.client(&Spec::testbin("threads"));
    let worker = client.clone();
    let pid = std::thread::spawn(move || {
        worker.fs_list("/").unwrap();
        worker.pid().unwrap()
    })
    .join()
    .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert!(proc_exists(pid));
    assert!(!client.fs_list("/").unwrap().is_empty());
    assert_eq!(client.pid(), Some(pid));
    assert_eq!(client.health().failures, 0);
}
