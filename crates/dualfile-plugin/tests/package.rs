//! Package checks: nothing of a plugin runs before its folder passed.

mod common;
use common::*;
use dualfile_plugin::package::{discover, verify_package_as};
use dualfile_plugin::{
    refuse_root, verify_package, Kind, PinnedState, PluginClient, PluginError, PluginRuntime,
    Session,
};
use serde_json::json;
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::path::Path;

fn chmod(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

fn unsafe_pkg(result: Result<dualfile_plugin::VerifiedPackage, PluginError>) -> bool {
    matches!(result, Err(PluginError::UnsafePackage(_)))
}

fn installed(spec: &Spec) -> (Env, std::path::PathBuf) {
    let env = Env::new();
    let dir = install(env.base(), spec);
    (env, dir)
}

#[test]
fn a_good_package_verifies_and_is_pinned() {
    let (env, _) = installed(&Spec::normal("good"));
    let package = verify_package(env.base(), "good").unwrap();
    assert_eq!(package.manifest.id, "good");
    assert!(package.manifest.has_kind(Kind::Filesystem));
    assert_eq!(package.manifest_sha256.len(), 64);
    let pinned = PinnedState::for_package(&package, false);
    pinned.check(&package).unwrap();
    // unknown or invalid ids are simply not installed
    assert!(matches!(
        verify_package(env.base(), "nope"),
        Err(PluginError::NotInstalled)
    ));
    assert!(matches!(
        verify_package(env.base(), "../good"),
        Err(PluginError::NotInstalled)
    ));
    assert!(matches!(
        verify_package(env.base(), "GOOD"),
        Err(PluginError::NotInstalled)
    ));
}

#[test]
fn symlinks_anywhere_in_the_path_are_refused() {
    let env = Env::new();
    // the plugin folder itself is a link
    let real = env.base().join("real-store");
    std::fs::create_dir(&real).unwrap();
    chmod(&real, 0o755);
    let spec = Spec::normal("linked");
    let inner = install(&real, &spec);
    symlink(&inner, env.base().join("linked")).unwrap();
    assert!(unsafe_pkg(verify_package(env.base(), "linked")));

    // a component of the base path is a link
    let alias = env.base().join("alias");
    symlink(&real, &alias).unwrap();
    assert!(unsafe_pkg(verify_package(&alias, "linked")));

    // the executable is a link
    let (env2, dir) = installed(&Spec::normal("exelink"));
    let target = env2.base().join("elsewhere");
    std::fs::rename(dir.join("example-plugin"), &target).unwrap();
    symlink(&target, dir.join("example-plugin")).unwrap();
    assert!(unsafe_pkg(verify_package(env2.base(), "exelink")));

    // the manifest is a link
    let (env3, dir) = installed(&Spec::normal("manilink"));
    let moved = env3.base().join("manifest-copy");
    std::fs::rename(dir.join("plugin.json"), &moved).unwrap();
    symlink(&moved, dir.join("plugin.json")).unwrap();
    assert!(unsafe_pkg(verify_package(env3.base(), "manilink")));

    // a link inside a sub folder of the package
    let (env4, dir) = installed(&Spec::normal("sublink"));
    symlink("/etc", dir.join("etc-link")).unwrap();
    assert!(unsafe_pkg(verify_package(env4.base(), "sublink")));
}

#[test]
fn files_and_folders_writable_by_others_are_refused() {
    for (what, path, mode) in [
        ("exe other", "example-plugin", 0o757),
        ("exe group", "example-plugin", 0o775),
        ("manifest other", "plugin.json", 0o646),
        ("manifest group", "plugin.json", 0o664),
        ("folder other", "", 0o757),
        ("folder group", "", 0o775),
    ] {
        let (env, dir) = installed(&Spec::normal("perm"));
        chmod(&dir.join(path), mode);
        assert!(unsafe_pkg(verify_package(env.base(), "perm")), "{what}");
    }
    // the plugin base folder
    let (env, _) = installed(&Spec::normal("perm"));
    chmod(env.base(), 0o777);
    assert!(unsafe_pkg(verify_package(env.base(), "perm")));
    chmod(env.base(), 0o700);
    verify_package(env.base(), "perm").unwrap();
    // an extra data file writable by others
    let (env, dir) = installed(&Spec::normal("perm").mode("x"));
    chmod(&dir.join("mode"), 0o666);
    assert!(unsafe_pkg(verify_package(env.base(), "perm")));
}

#[test]
fn files_of_another_owner_are_refused() {
    let (env, dir) = installed(&Spec::normal("owner"));
    let uid = std::fs::metadata(&dir).unwrap().uid();
    verify_package_as(env.base(), "owner", uid).unwrap();
    assert!(unsafe_pkg(verify_package_as(env.base(), "owner", uid + 1)));
    assert!(unsafe_pkg(verify_package_as(env.base(), "owner", 0)));
}

#[test]
fn hardlinked_files_and_special_files_are_refused() {
    let (env, dir) = installed(&Spec::normal("hard").mode("x"));
    std::fs::hard_link(dir.join("mode"), env.base().join("second-name")).unwrap();
    assert!(unsafe_pkg(verify_package(env.base(), "hard")));

    let (env, dir) = installed(&Spec::normal("fifo"));
    let status = std::process::Command::new("mkfifo")
        .arg(dir.join("pipe"))
        .status();
    if status.is_ok_and(|s| s.success()) {
        assert!(unsafe_pkg(verify_package(env.base(), "fifo")));
    }
}

#[test]
fn only_elf_programs_are_accepted() {
    let scratch = tempfile::tempdir().unwrap();
    for (name, content) in [
        ("shebang", b"#!/bin/sh\necho hi\n".to_vec()),
        ("text", b"just text".to_vec()),
        ("pe", b"MZ\x90\x00 not for linux".to_vec()),
        ("short", b"\x7fEL".to_vec()),
        ("empty", Vec::new()),
    ] {
        let binary = scratch.path().join(name);
        std::fs::write(&binary, &content).unwrap();
        let mut spec = Spec::normal("elf");
        spec.binary = binary;
        let (env, _) = installed(&spec);
        assert!(unsafe_pkg(verify_package(env.base(), "elf")), "{name}");
    }
    // no execute permission for the owner
    let (env, dir) = installed(&Spec::normal("noexec"));
    chmod(&dir.join("example-plugin"), 0o644);
    assert!(unsafe_pkg(verify_package(env.base(), "noexec")));
}

#[test]
fn any_difference_from_the_manifest_locks_the_plugin() {
    // an executable changed after the manifest was written
    let (env, dir) = installed(&Spec::normal("locked").mode("original"));
    let mut exe = std::fs::read(dir.join("example-plugin")).unwrap();
    exe.push(0);
    write_file(&dir.join("example-plugin"), &exe, 0o755);
    assert!(matches!(
        verify_package(env.base(), "locked"),
        Err(PluginError::Locked)
    ));

    // a listed data file changed
    let (env, dir) = installed(&Spec::normal("locked").mode("original"));
    write_file(&dir.join("mode"), b"changed", 0o644);
    assert!(matches!(
        verify_package(env.base(), "locked"),
        Err(PluginError::Locked)
    ));

    // a file nobody listed was added (also in a sub folder)
    let (env, dir) = installed(&Spec::normal("locked"));
    write_file(&dir.join("extra.so"), b"\x7fELF", 0o644);
    assert!(matches!(
        verify_package(env.base(), "locked"),
        Err(PluginError::Locked)
    ));
    let (env, dir) = installed(&Spec::normal("locked"));
    std::fs::create_dir(dir.join("lib")).unwrap();
    chmod(&dir.join("lib"), 0o755);
    write_file(&dir.join("lib/x.so"), b"x", 0o644);
    assert!(matches!(
        verify_package(env.base(), "locked"),
        Err(PluginError::Locked)
    ));

    // a listed file is missing
    let (env, dir) = installed(&Spec::normal("locked").mode("original"));
    std::fs::remove_file(dir.join("mode")).unwrap();
    assert!(matches!(
        verify_package(env.base(), "locked"),
        Err(PluginError::Locked)
    ));

    // the manifest names another id than its folder
    let (env, dir) = installed(&Spec::normal("locked"));
    std::fs::rename(&dir, env.base().join("other-name")).unwrap();
    assert!(matches!(
        verify_package(env.base(), "other-name"),
        Err(PluginError::Manifest(_))
    ));
}

#[test]
fn a_changed_manifest_no_longer_matches_what_the_user_confirmed() {
    let env = Env::new();
    let spec = Spec::normal("pin");
    let dir = install(env.base(), &spec);
    let first = verify_package(env.base(), "pin").unwrap();
    let pinned = PinnedState::for_package(&first, false);
    // the manifest changes (here: the write right is switched on) with all hashes valid
    let exe = std::fs::read(dir.join("example-plugin")).unwrap();
    let mut manifest = manifest_json(&spec, &exe);
    manifest["write"] = json!(true);
    write_file(
        &dir.join("plugin.json"),
        &serde_json::to_vec(&manifest).unwrap(),
        0o644,
    );
    let second = verify_package(env.base(), "pin").unwrap();
    assert!(second.manifest.write);
    assert!(matches!(pinned.check(&second), Err(PluginError::Locked)));
    assert!(matches!(
        PluginClient::open(env.runtime(), pinned.clone()),
        Err(PluginError::Locked)
    ));
    // the host also refuses to start it
    assert!(matches!(
        Session::start(&env.runtime(), &second, &pinned),
        Err(PluginError::Locked)
    ));
    // a pin for another id never matches
    let mut other = PinnedState::for_package(&second, false);
    other.id = "other".into();
    assert!(matches!(other.check(&second), Err(PluginError::Locked)));
    // confirming the new state makes it usable again
    PinnedState::for_package(&second, false)
        .check(&second)
        .unwrap();
}

#[test]
fn the_verified_descriptor_is_started_not_the_file_that_replaced_it() {
    let env = Env::new();
    let spec = Spec::normal("swap");
    let dir = install(env.base(), &spec);
    let package = verify_package(env.base(), "swap").unwrap();
    let pinned = PinnedState::for_package(&package, false);
    // after the check, another program (the test double with the misbehaving modes) takes
    // the place of the executable
    let replacement = dir.join("replacement");
    write_file(&replacement, &std::fs::read(TESTBIN).unwrap(), 0o755);
    std::fs::rename(&replacement, dir.join("example-plugin")).unwrap();
    assert_ne!(
        std::fs::read(dir.join("example-plugin")).unwrap(),
        std::fs::read(NORMAL).unwrap()
    );

    let runtime = env.runtime();
    let mut session = Session::start(&runtime, &package, &pinned).unwrap();
    // the original does not know the misbehaving modes; the replacement would
    let error = session
        .call(
            "debug_misbehave",
            serde_json::from_value(json!({"mode": "env"})).unwrap(),
            std::time::Duration::from_secs(5),
        )
        .unwrap_err();
    assert!(
        matches!(
            error,
            PluginError::Reported {
                code: dualfile_plugin::ErrorCode::Unsupported,
                ..
            }
        ),
        "{error:?}"
    );
    session.close();
    // a fresh check of the changed folder finds the difference
    assert!(matches!(
        verify_package(env.base(), "swap"),
        Err(PluginError::Locked)
    ));
}

#[test]
fn an_executable_changed_in_place_after_the_check_is_not_started() {
    use std::io::{Seek, SeekFrom, Write};
    let env = Env::new();
    let spec = Spec::normal("inplace");
    let dir = install(env.base(), &spec);
    let package = verify_package(env.base(), "inplace").unwrap();
    let pinned = PinnedState::for_package(&package, false);
    let path = dir.join("example-plugin");
    let size = std::fs::metadata(&path).unwrap().len();
    // same size, different content: overwrite the last byte in place
    let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.seek(SeekFrom::Start(size - 1)).unwrap();
    let last = std::fs::read(&path).unwrap()[size as usize - 1];
    file.write_all(&[last ^ 0xff]).unwrap();
    drop(file);
    assert_eq!(std::fs::metadata(&path).unwrap().len(), size);

    let runtime = env.runtime();
    let result = Session::start(&runtime, &package, &pinned);
    assert!(
        matches!(result, Err(PluginError::Locked)),
        "{:?}",
        result.err()
    );
}

#[test]
fn an_unchanged_executable_still_starts() {
    let env = Env::new();
    install(env.base(), &Spec::normal("intact"));
    let package = verify_package(env.base(), "intact").unwrap();
    let pinned = PinnedState::for_package(&package, false);
    let runtime = env.runtime();
    let session = Session::start(&runtime, &package, &pinned).unwrap();
    session.close();
}

#[test]
fn the_host_refuses_to_run_as_the_administrator() {
    assert!(matches!(refuse_root(0, 1000), Err(PluginError::Root)));
    assert!(matches!(refuse_root(1000, 0), Err(PluginError::Root)));
    assert!(matches!(refuse_root(0, 0), Err(PluginError::Root)));
    assert!(refuse_root(1000, 1000).is_ok());
}

#[test]
fn discovery_lists_good_and_locked_packages() {
    let env = Env::new();
    install(env.base(), &Spec::normal("aa-good"));
    let dir = install(env.base(), &Spec::normal("bb-bad"));
    write_file(&dir.join("example-plugin"), b"\x7fELFchanged", 0o755);
    std::fs::create_dir(env.base().join("Not_An_Id")).unwrap();
    let found = discover(env.base());
    let ids: Vec<&str> = found.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(ids, ["aa-good", "bb-bad"]);
    assert!(found[0].package.is_ok());
    assert!(matches!(found[1].package, Err(PluginError::Locked)));
    assert!(discover(&env.base().join("missing")).is_empty());
    // duplicate ids across manifests are refused by the manifest check
    let a = verify_package(env.base(), "aa-good").unwrap().manifest;
    assert!(dualfile_plugin::manifest::check_unique_ids(&[a.clone(), a]).is_err());
    let _ = PluginRuntime::new(env.config());
}
