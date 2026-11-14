//! `PluginVfs` as a `Vfs`: listing, reading, default read-only, controlled writing.

mod common;
use common::*;
use dualfile_core::error::Error;
use dualfile_core::ops::{self, JobHandle};
use dualfile_core::router::VfsRouter;
use dualfile_core::vfs::{EntryKind, ReplaceMode, Scheme, VPath, Vfs, WriteMode};
use dualfile_plugin::PluginVfs;
use std::io::{Read, Write};
use std::sync::Arc;

fn vfs_for(env: &Env, spec: &Spec) -> Arc<PluginVfs> {
    Arc::new(PluginVfs::with_write_grant(env.client(spec), true))
}

fn p(id: &str, path: &str) -> VPath {
    VPath::plugin(id, path).unwrap()
}

fn expected_big() -> Vec<u8> {
    (0..700_000u32)
        .map(|i| (i.wrapping_mul(7) % 251) as u8)
        .collect()
}

fn names(vfs: &PluginVfs, id: &str, dir: &str) -> Vec<String> {
    let mut names: Vec<String> = vfs
        .list(&p(id, dir))
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    names.sort();
    names
}

fn read_all(vfs: &PluginVfs, path: &VPath) -> Vec<u8> {
    let mut data = Vec::new();
    vfs.open_read(path).unwrap().read_to_end(&mut data).unwrap();
    data
}

#[test]
fn it_is_a_plugin_back_end_and_read_only_by_default() {
    let env = Env::new();
    let vfs = vfs_for(&env, &Spec::normal("ro"));
    assert_eq!(vfs.remote_endpoint(), Some((Scheme::Plugin, "ro")));
    assert!(vfs.is_read_only() && !vfs.is_local());
    let caps = vfs.capabilities();
    assert!(!caps.can_trash && !caps.can_rename_in_place);
    let target = p("ro", "/new.txt");
    assert!(matches!(
        vfs.create_write(&target, WriteMode::NoReplace),
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        vfs.mkdir(&p("ro", "/d")),
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        vfs.rename(
            &p("ro", "/hello.txt"),
            &p("ro", "/x.txt"),
            ReplaceMode::NoReplace
        ),
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        vfs.remove_permanent(&p("ro", "/hello.txt")),
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        vfs.trash(&p("ro", "/hello.txt")),
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        vfs.read_link(&p("ro", "/hello.txt")),
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        vfs.create_symlink("x", &p("ro", "/l")),
        Err(Error::Unsupported(_))
    ));
    // nothing changed
    assert_eq!(read_all(&vfs, &p("ro", "/hello.txt")), b"Hallo Welt\n");
    // a path of another plugin or of another scheme is not served
    assert!(matches!(
        vfs.list(&p("other", "/")),
        Err(Error::InvalidPath(_))
    ));
    assert!(matches!(
        vfs.list(&VPath::ftp("ro", "/").unwrap()),
        Err(Error::InvalidPath(_))
    ));
    assert!(matches!(
        vfs.list(&VPath::local("/tmp").unwrap()),
        Err(Error::InvalidPath(_))
    ));
}

#[test]
fn a_plugin_that_cannot_write_stays_read_only_even_with_the_right_in_the_manifest() {
    let env = Env::new();
    // the manifest allows writing, the (normal) program says it cannot
    let vfs = vfs_for(&env, &Spec::normal("cannot").writable());
    assert!(!vfs.is_read_only());
    assert!(matches!(
        vfs.create_write(&p("cannot", "/n.txt"), WriteMode::NoReplace),
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        vfs.mkdir(&p("cannot", "/d")),
        Err(Error::Unsupported(_))
    ));
    assert!(!names(&vfs, "cannot", "/").contains(&"n.txt".to_owned()));
}

#[test]
fn without_the_users_grant_even_a_writable_manifest_changes_nothing() {
    let env = Env::new();
    let client = env.client(&Spec::testbin("nogrant").writable());
    let vfs = PluginVfs::new(client.clone());
    assert!(vfs.is_read_only());
    assert!(!vfs.capabilities().can_rename_in_place);
    assert!(matches!(
        vfs.mkdir(&p("nogrant", "/d")),
        Err(Error::Unsupported(msg)) if msg == dualfile_core::plugins::MSG_PLUGIN_READ_ONLY
    ));
    assert!(matches!(
        vfs.remove_permanent(&p("nogrant", "/hello.txt")),
        Err(Error::Unsupported(_))
    ));
    // the same plugin with the grant may write
    let granted = PluginVfs::with_write_grant(client, true);
    assert!(!granted.is_read_only());
    granted.mkdir(&p("nogrant", "/d")).unwrap();
}

#[test]
fn listing_and_stat() {
    let env = Env::new();
    let vfs = vfs_for(&env, &Spec::normal("ls"));
    let root = vfs.list(&p("ls", "/")).unwrap();
    let hello = root.iter().find(|e| e.name == "hello.txt").unwrap();
    assert_eq!((hello.kind, hello.size), (EntryKind::File, 11));
    assert!(hello.modified.is_some() && hello.error.is_none() && !hello.hidden);
    assert_eq!(
        root.iter().find(|e| e.name == "docs").unwrap().kind,
        EntryKind::Dir
    );
    assert_eq!(names(&vfs, "ls", "/docs"), ["a.txt", "b.txt", "sub"]);
    assert_eq!(names(&vfs, "ls", "/docs/sub"), ["deep.txt"]);
    assert_eq!(vfs.list(&p("ls", "/many")).unwrap().len(), 1000);
    let stat = vfs.stat(&p("ls", "/docs/a.txt")).unwrap();
    assert_eq!((stat.name.as_str(), stat.size), ("a.txt", 1));
    assert_eq!(vfs.stat(&p("ls", "/docs")).unwrap().kind, EntryKind::Dir);
    assert!(matches!(
        vfs.stat(&p("ls", "/nope")),
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        vfs.list(&p("ls", "/nope")),
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        vfs.list(&p("ls", "/hello.txt")),
        Err(Error::NotADirectory(_))
    ));
    assert_eq!(vfs.stat_file(&p("ls", "/hello.txt")).unwrap().size, 11);
    assert!(matches!(
        vfs.stat_file(&p("ls", "/docs")),
        Err(Error::IsADirectory(_))
    ));
}

#[test]
fn hostile_and_colliding_names_are_marked_and_never_usable() {
    let env = Env::new();
    let vfs = vfs_for(&env, &Spec::normal("names"));
    let evil = vfs.list(&p("names", "/evil")).unwrap();
    assert_eq!(evil.len(), 7);
    for entry in &evil {
        // what is shown never carries a separator or a control character
        assert!(
            !entry.name.contains('/')
                && !entry.name.contains('\\')
                && !entry.name.chars().any(char::is_control),
            "{:?}",
            entry.name
        );
    }
    let usable: Vec<&str> = evil
        .iter()
        .filter(|e| e.error.is_none())
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(usable, ["ok.txt"]);
    assert_eq!(evil.iter().filter(|e| e.error.is_some()).count(), 6);
    // case-only collisions mark both members
    let dup = vfs.list(&p("names", "/dup")).unwrap();
    assert_eq!(dup.len(), 2);
    assert!(dup.iter().all(|e| e.error.is_some()));
    // a marked file cannot be read, even by its real path
    assert!(matches!(
        vfs.open_read(&p("names", "/dup/Case.txt")),
        Err(Error::InvalidName(_))
    ));
    assert_eq!(read_all(&vfs, &p("names", "/evil/ok.txt")), b"ok");
    // the program's own checks refuse hostile names in paths as well
    assert!(VPath::plugin("names", "/evil/a\nb").is_err());
    assert!(p("names", "/evil").join("..").is_err());
}

#[test]
fn reading_whole_files_ranges_and_from_an_offset() {
    let env = Env::new();
    let vfs = vfs_for(&env, &Spec::normal("rd"));
    let big = expected_big();
    // more than two chunks, delivered in pieces
    assert_eq!(read_all(&vfs, &p("rd", "/big.bin")), big);
    assert_eq!(
        vfs.read_range(&p("rd", "/big.bin"), 100, 50).unwrap(),
        &big[100..150]
    );
    assert_eq!(
        vfs.read_range(&p("rd", "/big.bin"), 262_100, 1000).unwrap(),
        &big[262_100..263_100]
    );
    assert_eq!(
        vfs.read_range(&p("rd", "/big.bin"), 699_990, 100).unwrap(),
        &big[699_990..]
    );
    assert!(vfs
        .read_range(&p("rd", "/big.bin"), 800_000, 10)
        .unwrap()
        .is_empty());
    // a range spanning several chunks
    assert_eq!(
        vfs.read_range(&p("rd", "/big.bin"), 10, 600_000).unwrap(),
        &big[10..600_010]
    );
    let mut tail = Vec::new();
    vfs.open_read_from(&p("rd", "/big.bin"), 650_000)
        .unwrap()
        .read_to_end(&mut tail)
        .unwrap();
    assert_eq!(tail, &big[650_000..]);
    assert!(matches!(
        vfs.open_read(&p("rd", "/docs")),
        Err(Error::IsADirectory(_))
    ));
    assert!(matches!(
        vfs.open_read(&p("rd", "/missing")),
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        vfs.read_range(&p("rd", "/docs"), 0, 1),
        Err(Error::IsADirectory(_))
    ));
}

#[test]
fn plugin_text_never_reaches_error_messages() {
    let env = Env::new();
    let vfs = vfs_for(&env, &Spec::normal("msg"));
    // the plugin answers with the text "is a file"; the message is the program's own
    let error = vfs.list(&p("msg", "/hello.txt")).unwrap_err();
    let shown = error.to_string();
    assert!(
        !shown.contains("is a file") && shown.contains("/hello.txt"),
        "{shown}"
    );
    // failures of the process give fixed phrases
    let spec = Spec::testbin("crashy");
    let vfs = vfs_for(&env, &spec);
    vfs.client()
        .raw_call("debug_misbehave", serde_json::json!({"mode": "crash"}))
        .unwrap_err();
    vfs.client().reset_health();
    assert!(vfs.list(&p("crashy", "/")).is_ok());
}

#[test]
fn writing_goes_through_a_helper_file_and_never_replaces_without_being_asked() {
    let env = Env::new();
    let vfs = vfs_for(&env, &Spec::testbin("wr").writable());
    assert!(!vfs.is_read_only() && vfs.capabilities().can_rename_in_place);
    let target = p("wr", "/new.bin");
    let data = expected_big();
    let mut writer = vfs.create_write(&target, WriteMode::NoReplace).unwrap();
    writer.write_all(&data[..300_000]).unwrap();
    writer.write_all(&data[300_000..]).unwrap();
    // while writing, only the helper file exists (hidden, never the final name)
    let during = names(&vfs, "wr", "/");
    assert!(!during.contains(&"new.bin".to_owned()), "{during:?}");
    assert!(
        during.iter().any(|n| n.starts_with(".new.bin.dfpart-")),
        "{during:?}"
    );
    writer.commit().unwrap();
    drop(writer);
    let after = names(&vfs, "wr", "/");
    assert!(
        after.contains(&"new.bin".to_owned()) && !after.iter().any(|n| n.contains("dfpart")),
        "{after:?}"
    );
    assert_eq!(read_all(&vfs, &target), data);

    // an existing target is never replaced in NoReplace mode, and nothing is left behind
    assert!(matches!(
        vfs.create_write(&target, WriteMode::NoReplace),
        Err(Error::Exists(_))
    ));
    assert!(matches!(
        vfs.create_write(&p("wr", "/hello.txt"), WriteMode::NoReplace),
        Err(Error::Exists(_))
    ));
    assert_eq!(read_all(&vfs, &p("wr", "/hello.txt")), b"Hallo Welt\n");
    // replacing is an explicit mode
    let mut writer = vfs
        .create_write(&p("wr", "/hello.txt"), WriteMode::Replace)
        .unwrap();
    writer.write_all(b"new text").unwrap();
    writer.commit().unwrap();
    drop(writer);
    assert_eq!(read_all(&vfs, &p("wr", "/hello.txt")), b"new text");
    // a folder is never replaced by a file
    assert!(matches!(
        vfs.create_write(&p("wr", "/docs"), WriteMode::Replace),
        Err(Error::IsADirectory(_))
    ));
    // a bad name is refused before anything happens
    assert!(VPath::plugin("wr", "/a\nb").is_err());
    assert!(!names(&vfs, "wr", "/").iter().any(|n| n.contains("dfpart")));
}

#[test]
fn a_write_that_is_not_committed_leaves_nothing_behind() {
    let env = Env::new();
    let vfs = vfs_for(&env, &Spec::testbin("abort").writable());
    {
        let mut writer = vfs
            .create_write(&p("abort", "/half.txt"), WriteMode::NoReplace)
            .unwrap();
        writer.write_all(&[7u8; 400_000]).unwrap();
        // dropped without commit
    }
    let after = names(&vfs, "abort", "/");
    assert!(
        !after.contains(&"half.txt".to_owned()) && !after.iter().any(|n| n.contains("dfpart")),
        "{after:?}"
    );
    // a file that appeared while writing wins: the commit refuses and cleans up
    let mut writer = vfs
        .create_write(&p("abort", "/race.txt"), WriteMode::NoReplace)
        .unwrap();
    writer.write_all(b"mine").unwrap();
    let other = vfs
        .client()
        .raw_call(
            "fs.write_open",
            serde_json::json!({"path": "/race.txt", "replace": false}),
        )
        .unwrap();
    vfs.client()
        .raw_call(
            "fs.write_close",
            serde_json::json!({"handle": other["handle"], "commit": true}),
        )
        .unwrap();
    let committed = writer.commit();
    assert!(matches!(committed, Err(Error::Exists(_))), "{committed:?}");
    drop(writer);
    assert!(!names(&vfs, "abort", "/")
        .iter()
        .any(|n| n.contains("dfpart")));
}

#[test]
fn a_write_handle_does_not_survive_a_restart_of_the_plugin() {
    let env = Env::new();
    let vfs = vfs_for(&env, &Spec::testbin("restart").writable());
    let mut writer = vfs
        .create_write(&p("restart", "/x.txt"), WriteMode::NoReplace)
        .unwrap();
    writer.write_all(&[1u8; 300_000]).unwrap();
    vfs.client()
        .raw_call("debug_misbehave", serde_json::json!({"mode": "crash"}))
        .unwrap_err();
    vfs.client().reset_health();
    // the new process knows nothing of the old handle: the write fails, nothing appears
    let result = writer
        .write_all(&[1u8; 300_000])
        .and_then(|()| writer.commit().map_err(std::io::Error::other));
    assert!(result.is_err());
    drop(writer);
    assert!(!names(&vfs, "restart", "/").contains(&"x.txt".to_owned()));
}

#[test]
fn mkdir_rename_and_permanent_removal() {
    let env = Env::new();
    let vfs = vfs_for(&env, &Spec::testbin("chg").writable());
    vfs.mkdir(&p("chg", "/made")).unwrap();
    assert_eq!(vfs.stat(&p("chg", "/made")).unwrap().kind, EntryKind::Dir);
    assert!(matches!(
        vfs.mkdir(&p("chg", "/made")),
        Err(Error::Exists(_))
    ));
    assert!(matches!(
        vfs.mkdir(&p("chg", "/docs")),
        Err(Error::Exists(_))
    ));
    // rename without replacing
    assert!(matches!(
        vfs.rename(
            &p("chg", "/docs/a.txt"),
            &p("chg", "/docs/b.txt"),
            ReplaceMode::NoReplace
        ),
        Err(Error::Exists(_))
    ));
    assert_eq!(read_all(&vfs, &p("chg", "/docs/b.txt")), b"B");
    vfs.rename(
        &p("chg", "/docs/a.txt"),
        &p("chg", "/docs/c.txt"),
        ReplaceMode::NoReplace,
    )
    .unwrap();
    assert!(matches!(
        vfs.stat(&p("chg", "/docs/a.txt")),
        Err(Error::NotFound(_))
    ));
    assert_eq!(read_all(&vfs, &p("chg", "/docs/c.txt")), b"A");
    assert!(matches!(
        vfs.rename(
            &p("chg", "/docs/zzz"),
            &p("chg", "/docs/y"),
            ReplaceMode::NoReplace
        ),
        Err(Error::NotFound(_))
    ));
    vfs.rename(
        &p("chg", "/docs/c.txt"),
        &p("chg", "/docs/b.txt"),
        ReplaceMode::Replace,
    )
    .unwrap();
    assert_eq!(read_all(&vfs, &p("chg", "/docs/b.txt")), b"A");
    // remove: a folder with content is refused, an empty one and a file go
    assert!(matches!(
        vfs.remove_permanent(&p("chg", "/docs")),
        Err(Error::DirectoryNotEmpty(_))
    ));
    vfs.remove_permanent(&p("chg", "/made")).unwrap();
    vfs.remove_permanent(&p("chg", "/docs/b.txt")).unwrap();
    assert!(matches!(
        vfs.remove_permanent(&p("chg", "/docs/b.txt")),
        Err(Error::NotFound(_))
    ));
    // trash is not a thing here and never deletes
    assert!(matches!(
        vfs.trash(&p("chg", "/hello.txt")),
        Err(Error::Unsupported(_))
    ));
    assert!(vfs.stat(&p("chg", "/hello.txt")).is_ok());
}

#[test]
fn the_programs_jobs_copy_and_delete_with_their_usual_safety() {
    let env = Env::new();
    let vfs = vfs_for(&env, &Spec::testbin("jobs").writable());
    let router = VfsRouter::new();
    router.register_plugin("jobs", vfs.clone()).unwrap();
    let local = tempfile::tempdir().unwrap();
    let local_dir = VPath::local(local.path()).unwrap();
    let job = JobHandle::silent();

    // plugin -> local
    let source = p("jobs", "/big.bin");
    let report = ops::copy(
        &*router.vfs_for(&source).unwrap(),
        std::slice::from_ref(&source),
        &dualfile_core::LocalVfs,
        &local_dir,
        None,
        &job,
    );
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(
        std::fs::read(local.path().join("big.bin")).unwrap(),
        expected_big()
    );

    // local -> plugin, an existing target is not replaced without a decision
    std::fs::write(local.path().join("up.txt"), b"uploaded").unwrap();
    let up = VPath::local(local.path().join("up.txt")).unwrap();
    let dest = p("jobs", "/docs");
    let to_plugin = router.vfs_for(&dest).unwrap();
    let report = ops::copy(
        &dualfile_core::LocalVfs,
        std::slice::from_ref(&up),
        &*to_plugin,
        &dest,
        None,
        &job,
    );
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(read_all(&vfs, &p("jobs", "/docs/up.txt")), b"uploaded");
    std::fs::write(local.path().join("up.txt"), b"second").unwrap();
    let report = ops::copy(
        &dualfile_core::LocalVfs,
        std::slice::from_ref(&up),
        &*to_plugin,
        &dest,
        None,
        &job,
    );
    assert_eq!(
        report.skipped.len(),
        1,
        "no resolver: the existing file is skipped"
    );
    assert_eq!(read_all(&vfs, &p("jobs", "/docs/up.txt")), b"uploaded");

    // trash through the job layer: refused, nothing removed
    let victim = p("jobs", "/docs/up.txt");
    let report = ops::trash_items(&*to_plugin, std::slice::from_ref(&victim), &job);
    assert!(matches!(report.failed[0].1, Error::Unsupported(_)));
    assert!(vfs.stat(&victim).is_ok());

    // deleting only works through the delete plan and its token
    let plan = ops::plan_delete(&*to_plugin, std::slice::from_ref(&victim));
    let wrong = ops::delete_permanent(&*to_plugin, &plan, plan.token.wrapping_add(1), &job);
    assert!(!wrong.failed.is_empty() || wrong.done.is_empty());
    assert!(vfs.stat(&victim).is_ok(), "a wrong token deletes nothing");
    let report = ops::delete_permanent(&*to_plugin, &plan, plan.token, &job);
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert!(matches!(vfs.stat(&victim), Err(Error::NotFound(_))));

    // moving out of a plugin copies, checks, then removes the source through the plugin
    let moved = p("jobs", "/hello.txt");
    let report = ops::move_(
        &*to_plugin,
        std::slice::from_ref(&moved),
        &dualfile_core::LocalVfs,
        &local_dir,
        None,
        &job,
    );
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(
        std::fs::read(local.path().join("hello.txt")).unwrap(),
        b"Hallo Welt\n"
    );
    assert!(matches!(vfs.stat(&moved), Err(Error::NotFound(_))));
}

#[test]
fn the_router_serves_plugin_paths_only_for_registered_plugins() {
    let env = Env::new();
    let vfs = vfs_for(&env, &Spec::normal("route"));
    let router = VfsRouter::new();
    assert!(matches!(
        router.vfs_for(&p("route", "/")),
        Err(Error::Disconnected)
    ));
    router.register_plugin("route", vfs.clone()).unwrap();
    let found = router.vfs_for(&p("route", "/hello.txt")).unwrap();
    assert_eq!(found.remote_endpoint(), Some((Scheme::Plugin, "route")));
    assert!(router.register_plugin("other", vfs.clone()).is_err());
    assert!(router.unregister_plugin("route").is_some());
    assert!(matches!(
        router.vfs_for(&p("route", "/")),
        Err(Error::Disconnected)
    ));
}
