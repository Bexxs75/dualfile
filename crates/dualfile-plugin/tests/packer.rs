//! A plugin packer behind the router: archives of the declared extension open through the
//! archive layer with all of its safety rules.

mod common;
use common::*;
use dualfile_core::archive::PackerBinding;
use dualfile_core::error::Error;
use dualfile_core::router::VfsRouter;
use dualfile_core::vfs::{EntryKind, VPath, Vfs};
use dualfile_plugin::PluginPacker;
use std::io::Read;
use std::sync::Arc;

fn router_with_packer(env: &Env) -> (VfsRouter, Arc<dualfile_plugin::PluginClient>) {
    let client = env.client(&Spec::normal("lines"));
    let router = VfsRouter::new();
    let binding = PackerBinding::new(
        client.id(),
        client
            .manifest()
            .packer
            .as_ref()
            .unwrap()
            .extensions
            .clone(),
        Arc::new(PluginPacker::new(client.clone())),
    )
    .unwrap();
    router.register_packer(binding).unwrap();
    (router, client)
}

fn read(vfs: &dyn Vfs, path: &VPath) -> Vec<u8> {
    let mut data = Vec::new();
    vfs.open_read(path).unwrap().read_to_end(&mut data).unwrap();
    data
}

#[test]
fn archives_of_the_plugins_extension_list_and_read_through_the_plugin() {
    if dualfile_plugin::sandbox::probe().is_err() {
        eprintln!("skipped: no sandbox on this system");
        return;
    }
    let env = Env::new();
    let (router, client) = router_with_packer(&env);
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("a.xyz");
    std::fs::write(
        &file,
        "docs/a.txt\tAAA\n../evil.txt\tX\n/abs.txt\tX\nback\\slash\tX\nb.txt\tBB\n",
    )
    .unwrap();
    let loc = VPath::local(&file).unwrap();
    let root = VPath::archive(loc.clone(), "/").unwrap();
    let vfs = router.vfs_for(&root).unwrap();
    assert!(vfs.is_read_only());
    let entries = vfs.list(&root).unwrap();
    let safe: Vec<&str> = entries
        .iter()
        .filter(|e| e.error.is_none())
        .map(|e| e.name.as_str())
        .collect();
    assert!(
        safe.contains(&"docs") && safe.contains(&"b.txt"),
        "{safe:?}"
    );
    let unsafe_entries: Vec<_> = entries.iter().filter(|e| e.error.is_some()).collect();
    assert_eq!(unsafe_entries.len(), 3, "{entries:?}");
    assert!(unsafe_entries
        .iter()
        .all(|e| e.name.starts_with("[unsicher")));
    let a = VPath::archive(loc.clone(), "/docs/a.txt").unwrap();
    assert_eq!(vfs.stat(&a).unwrap().kind, EntryKind::File);
    assert_eq!(read(&*vfs, &a), b"AAA");
    assert_eq!(vfs.read_range(&a, 1, 5).unwrap(), b"AA");
    // an unsafe entry is never readable
    let bad = VPath::archive(loc, &format!("/{}", unsafe_entries[0].name)).unwrap();
    assert!(vfs.open_read(&bad).is_err());
    // writing into it is refused
    assert!(vfs
        .mkdir(&VPath::archive(root.outer().unwrap().clone(), "/n").unwrap())
        .is_err());
    assert_eq!(client.health().failures, 0);
}

#[test]
fn an_archive_above_the_plugin_limit_gets_a_clear_message() {
    let env = Env::new();
    let (router, _client) = router_with_packer(&env);
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("big.xyz");
    std::fs::write(
        &file,
        vec![b'x'; dualfile_plugin::protocol::MAX_PACKER_INPUT + 1],
    )
    .unwrap();
    let root = VPath::archive(VPath::local(&file).unwrap(), "/").unwrap();
    match router.vfs_for(&root) {
        Err(Error::Archive(message)) => assert!(message.contains("zu groß"), "{message}"),
        other => panic!("expected an archive error, got {:?}", other.err()),
    }
}

#[test]
fn built_in_formats_win_over_the_plugin() {
    let env = Env::new();
    let (router, client) = router_with_packer(&env);
    let tmp = tempfile::tempdir().unwrap();
    // a ZIP signature with the plugin's extension: read by the built-in ZIP code, the plugin
    // is never started for it
    let mut zip = Vec::new();
    {
        use std::io::{Cursor, Write};
        let mut writer = zip::ZipWriter::new(Cursor::new(&mut zip));
        writer
            .start_file("f.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"zip").unwrap();
        writer.finish().unwrap();
    }
    let file = tmp.path().join("real.xyz");
    std::fs::write(&file, zip).unwrap();
    let f = VPath::archive(VPath::local(&file).unwrap(), "/f.txt").unwrap();
    assert_eq!(read(&*router.vfs_for(&f).unwrap(), &f), b"zip");
    assert!(!client.health().running);
}
