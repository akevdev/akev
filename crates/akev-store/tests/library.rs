//! The session library (named sessions on disk) and the store operations behind autosave.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use akev_core::*;
use akev_store::{
    AUTOSAVE_ID, AUTOSAVE_PREVIOUS_ID, ExchangePatch, LibraryError, OpenMode, SessionKind,
    SessionLibrary, SessionLimits, Store, StoreEvent,
};
use common::*;
use std::sync::Arc;

fn populated(n: u64) -> Arc<Store> {
    let store = store();
    for i in 1..=n {
        complete_exchange(
            &store,
            i,
            &format!("https://api.example/items/{i}"),
            "application/json",
            format!("{{\"item\":{i}}}").as_bytes(),
        );
    }
    store
}

fn library() -> (tempfile::TempDir, SessionLibrary) {
    let dir = tempfile::tempdir().unwrap();
    let lib = SessionLibrary::new(dir.path().join("sessions"));
    (dir, lib)
}

fn paths(store: &Store) -> Vec<String> {
    store.summaries().into_iter().map(|s| s.path).collect()
}

#[test]
fn save_list_rename_delete() {
    let (_dir, lib) = library();
    assert!(
        lib.list().unwrap().is_empty(),
        "missing directory lists as empty"
    );
    let store = populated(3);

    let first = lib.save(&store, "  Checkout bug  ", None).unwrap();
    assert_eq!(first.id, "checkout-bug");
    assert_eq!(first.name, "Checkout bug");
    assert_eq!(first.kind, SessionKind::Saved);
    assert_eq!(first.exchanges, 3);
    assert!(first.size > 0);
    assert_eq!(first.created_at, first.updated_at);
    assert!(lib.dir().join("checkout-bug.akev").is_file());
    assert!(lib.dir().join("checkout-bug.json").is_file());

    // Same name again: a second session with a distinct id; a subset of the traffic.
    std::thread::sleep(std::time::Duration::from_millis(5));
    let second = lib
        .save(
            &store,
            "Checkout bug",
            Some(&[ExchangeId(2), ExchangeId(404)]),
        )
        .unwrap();
    assert_eq!(second.id, "checkout-bug-2");
    assert_eq!(second.exchanges, 1);

    let list = lib.list().unwrap();
    assert_eq!(
        list.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        ["checkout-bug-2", "checkout-bug"],
        "most recently updated first"
    );
    assert_eq!(lib.info("checkout-bug").unwrap(), first);

    // Rename keeps the id (and the contents).
    let renamed = lib.rename("checkout-bug", "Payment flow").unwrap();
    assert_eq!(
        (renamed.id.as_str(), renamed.name.as_str()),
        ("checkout-bug", "Payment flow")
    );
    assert_eq!(renamed.exchanges, 3);
    assert_eq!(lib.info("checkout-bug").unwrap().name, "Payment flow");
    assert!(matches!(
        lib.rename("checkout-bug", " "),
        Err(LibraryError::Invalid(_))
    ));
    assert!(matches!(
        lib.rename("nope", "x"),
        Err(LibraryError::NotFound)
    ));

    lib.delete("checkout-bug-2").unwrap();
    assert!(!lib.dir().join("checkout-bug-2.json").exists());
    assert!(matches!(
        lib.delete("checkout-bug-2"),
        Err(LibraryError::NotFound)
    ));
    assert_eq!(lib.list().unwrap().len(), 1);

    // Ids can't escape the directory.
    for bad in ["../x", "a/b", "", "UPPER", ".hidden"] {
        assert!(
            matches!(lib.info(bad), Err(LibraryError::NotFound)),
            "{bad}"
        );
        assert!(
            matches!(lib.delete(bad), Err(LibraryError::NotFound)),
            "{bad}"
        );
    }
    assert!(matches!(
        lib.save(&store, "", None),
        Err(LibraryError::Invalid(_))
    ));
}

#[test]
fn open_replaces_or_appends() {
    let (_dir, lib) = library();
    let source = populated(3);
    source.patch(
        &[ExchangeId(1)],
        &ExchangePatch {
            note: Some(Some("keep me".into())),
            ..ExchangePatch::default()
        },
    );
    let saved = lib.save(&source, "Three", None).unwrap();

    // Replace: everything goes, pinned exchanges too.
    let target = store();
    complete_exchange(&target, 100, "https://other.example/x", "text/plain", b"x");
    target.patch(
        &[ExchangeId(100)],
        &ExchangePatch {
            pinned: Some(true),
            ..ExchangePatch::default()
        },
    );
    let mut events = target.subscribe();
    let ids = lib.open(&saved.id, &target, OpenMode::Replace).unwrap();
    assert_eq!(ids.len(), 3);
    assert_eq!(paths(&target), ["/items/1", "/items/2", "/items/3"]);
    assert_eq!(
        target.detail(ids[0]).unwrap().note.as_deref(),
        Some("keep me")
    );
    let kinds = kinds(&drain(&mut events));
    assert_eq!(kinds.first(), Some(&"cleared"));
    assert_eq!(kinds.iter().filter(|k| **k == "added").count(), 3);

    // Append keeps what's there.
    let ids = lib.open(&saved.id, &target, OpenMode::Append).unwrap();
    assert_eq!(ids.len(), 3);
    assert_eq!(target.stats().exchanges, 6);

    // A corrupt session never touches the traffic.
    std::fs::write(lib.dir().join("broken.akev"), b"definitely not a session").unwrap();
    assert!(matches!(
        lib.open("broken", &target, OpenMode::Replace),
        Err(LibraryError::Store(_))
    ));
    assert_eq!(target.stats().exchanges, 6);
    assert!(matches!(
        lib.open("missing", &target, OpenMode::Append),
        Err(LibraryError::NotFound)
    ));
    // Unreadable files aren't listed.
    assert_eq!(lib.list().unwrap().len(), 1);
}

#[test]
fn autosave_overwrites_in_place_and_rotates() {
    let (_dir, lib) = library();
    let store = populated(2);
    let a = lib.autosave(&store, None).unwrap();
    assert_eq!((a.id.as_str(), a.name.as_str()), (AUTOSAVE_ID, "Autosave"));
    assert_eq!(a.kind, SessionKind::Autosave);
    assert_eq!(a.exchanges, 2);

    complete_exchange(
        &store,
        3,
        "https://api.example/items/3",
        "application/json",
        b"{}",
    );
    std::thread::sleep(std::time::Duration::from_millis(5));
    let b = lib.autosave(&store, None).unwrap();
    assert_eq!(b.exchanges, 3);
    assert_eq!(b.created_at, a.created_at, "first save time is kept");
    assert!(b.updated_at > a.updated_at);
    assert_eq!(lib.list().unwrap().len(), 1);
    // No temp files are left behind.
    let leftovers: Vec<_> = std::fs::read_dir(lib.dir())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with('.'))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");

    // Rotation keeps the previous run's autosave out of the next autosave's way.
    assert!(lib.rotate().unwrap());
    assert!(matches!(lib.info(AUTOSAVE_ID), Err(LibraryError::NotFound)));
    let prev = lib.info(AUTOSAVE_PREVIOUS_ID).unwrap();
    assert_eq!(prev.name, "Autosave (previous run)");
    assert_eq!(prev.exchanges, 3);
    assert_eq!(prev.kind, SessionKind::Autosave);
    assert!(!lib.rotate().unwrap(), "nothing to rotate");

    // Renaming an autosave keeps it as a regular session.
    let kept = lib
        .rename(AUTOSAVE_PREVIOUS_ID, "Friday's traffic")
        .unwrap();
    assert_eq!(kept.id, "friday-s-traffic");
    assert_eq!(kept.kind, SessionKind::Saved);
    assert_eq!(kept.exchanges, 3);
    assert!(matches!(
        lib.info(AUTOSAVE_PREVIOUS_ID),
        Err(LibraryError::NotFound)
    ));
    let reopened = common::store();
    assert_eq!(
        lib.open(&kept.id, &reopened, OpenMode::Append)
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn sessions_without_sidecars_are_described_from_their_contents() {
    let (_dir, lib) = library();
    let store = populated(4);
    let saved = lib.save(&store, "Copied in", None).unwrap();
    let sidecar = lib.dir().join(format!("{}.json", saved.id));
    std::fs::remove_file(&sidecar).unwrap();
    let info = lib.info(&saved.id).unwrap();
    assert_eq!(info.name, "Copied in", "name from the session header");
    assert_eq!(info.exchanges, 4, "exchange lines counted");
    assert!(sidecar.exists(), "sidecar recreated");

    // A file replaced behind the library's back (size differs) is rescanned.
    let mut smaller = Vec::new();
    store
        .save_session(&mut smaller, Some(&[ExchangeId(1)]))
        .unwrap();
    std::fs::write(lib.dir().join(format!("{}.akev", saved.id)), &smaller).unwrap();
    let info = lib.info(&saved.id).unwrap();
    assert_eq!(info.exchanges, 1);
    assert_eq!(info.size, smaller.len() as u64);
    assert_eq!(
        info.name, saved.id,
        "plain exports have no name: the id is used"
    );

    // Plain `.akev` exports dropped into the directory are listed too.
    std::fs::write(lib.dir().join("export.akev"), &smaller).unwrap();
    assert!(lib.list().unwrap().iter().any(|s| s.id == "export"));
}

#[test]
fn stale_temp_files_are_cleaned_up() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let stale = sessions.join(".tmp-old.part");
    let fresh = sessions.join(".tmp-new.part");
    std::fs::write(&stale, b"x").unwrap();
    std::fs::write(&fresh, b"x").unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
    std::fs::File::options()
        .write(true)
        .open(&stale)
        .unwrap()
        .set_modified(old)
        .unwrap();
    let lib = SessionLibrary::new(&sessions);
    assert!(!stale.exists());
    assert!(
        fresh.exists(),
        "a concurrent write in progress is left alone"
    );
    assert!(lib.list().unwrap().is_empty());
}

#[test]
fn generation_tracks_visible_changes() {
    let store = populated(1);
    let g0 = store.generation();
    assert_eq!(store.generation(), g0, "reads don't change it");
    complete_exchange(&store, 2, "https://api.example/2", "text/plain", b"x");
    let g1 = store.generation();
    assert!(g1 > g0);
    store.patch(
        &[ExchangeId(2)],
        &ExchangePatch {
            pinned: Some(true),
            ..ExchangePatch::default()
        },
    );
    let g2 = store.generation();
    assert!(g2 > g1);
    store.delete(&[ExchangeId(1)]);
    assert!(store.generation() > g2);
}

#[test]
fn autosave_bounds_keep_the_newest_and_all_pinned() {
    let store = store();
    for i in 1..=10u64 {
        let body = vec![b'x'; 100];
        complete_exchange(
            &store,
            i,
            &format!("https://b.example/{i}"),
            "text/plain",
            &body,
        );
    }
    store.patch(
        &[ExchangeId(2)],
        &ExchangePatch {
            pinned: Some(true),
            ..ExchangePatch::default()
        },
    );
    assert_eq!(store.newest_within(0, 0), None, "unbounded");
    assert_eq!(store.newest_within(100, 1 << 30), None, "everything fits");
    let ids = |v: Option<Vec<ExchangeId>>| v.unwrap().into_iter().map(|i| i.0).collect::<Vec<_>>();
    assert_eq!(ids(store.newest_within(3, 0)), [2, 8, 9, 10]);
    // 100-byte bodies: 350 bytes fit three unpinned exchanges.
    assert_eq!(ids(store.newest_within(0, 350)), [2, 8, 9, 10]);
    assert_eq!(ids(store.newest_within(2, 350)), [2, 9, 10]);
}

#[test]
fn reserved_ids_sort_before_traffic_captured_meanwhile() {
    let source = populated(3);
    let mut file = Vec::new();
    source.save_session(&mut file, None).unwrap();

    let store = store();
    let first = ExchangeId::reserve(2);
    // Live traffic arrives while the session loads.
    let live = ExchangeId::next();
    complete_exchange(
        &store,
        live.0,
        "https://live.example/now",
        "text/plain",
        b"x",
    );
    let mut events = store.subscribe();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.akev");
    std::fs::write(&path, &file).unwrap();
    let ids = store
        .load_session_file(
            &path,
            SessionLimits::default(),
            OpenMode::Append,
            Some((first, 2)),
        )
        .unwrap();
    assert_eq!(ids[..2], [first, ExchangeId(first.0 + 1)]);
    assert!(ids[2] > live, "beyond the reservation: fresh ids");
    let order: Vec<String> = paths(&store);
    assert_eq!(order, ["/items/1", "/items/2", "/now", "/items/3"]);
    let added = drain(&mut events)
        .into_iter()
        .filter(|e| matches!(e, StoreEvent::Added(_)))
        .count();
    assert_eq!(added, 3);
}

#[test]
fn named_sessions_still_load_as_plain_imports() {
    let (_dir, lib) = library();
    let source = populated(2);
    let saved = lib.save(&source, "Named", None).unwrap();
    let bytes = std::fs::read(lib.dir().join(format!("{}.akev", saved.id))).unwrap();
    let target = store();
    assert_eq!(target.load_session(&mut bytes.as_slice()).unwrap().len(), 2);
}

#[test]
fn large_sessions_load_in_batches_within_the_memory_budget() {
    let (_dir, lib) = library();
    let source = store();
    for i in 1..=600u64 {
        let body = noise(8 * 1024, i as u32);
        complete_exchange(
            &source,
            i,
            &format!("https://big.example/{i}"),
            "application/octet-stream",
            &body,
        );
    }
    let saved = lib.save(&source, "Big", None).unwrap();
    assert_eq!(saved.exchanges, 600);

    // ~4.7 MB of bodies into a 1 MiB budget: batches spill as they go.
    let target = store_with(|c| c.memory_budget_bytes = 1024 * 1024);
    let ids = lib.open(&saved.id, &target, OpenMode::Replace).unwrap();
    assert_eq!(ids.len(), 600);
    assert!(ids.windows(2).all(|w| w[0] < w[1]), "file order kept");
    let stats = target.stats();
    assert!(stats.memory_body_bytes <= 1024 * 1024, "{stats:?}");
    assert!(stats.spilled_body_bytes > 3 * 1024 * 1024, "{stats:?}");
    assert_eq!(
        target
            .body(ids[599], Side::Response)
            .unwrap()
            .bytes
            .as_ref(),
        noise(8 * 1024, 600).as_slice()
    );
}

#[test]
fn a_damaged_tail_inserts_nothing_and_keeps_the_traffic() {
    let (_dir, lib) = library();
    let source = populated(300);
    let saved = lib.save(&source, "Damaged", None).unwrap();
    // Decompress, cut the last line short, store it uncompressed (also accepted).
    let file = std::fs::read(lib.dir().join(format!("{}.akev", saved.id))).unwrap();
    let text = zstd::stream::decode_all(file.as_slice()).unwrap();
    let cut = text.len() - 40;
    std::fs::write(lib.dir().join(format!("{}.akev", saved.id)), &text[..cut]).unwrap();

    let target = populated(2);
    let err = lib.open(&saved.id, &target, OpenMode::Replace).unwrap_err();
    assert!(matches!(err, LibraryError::Store(_)), "{err}");
    assert_eq!(
        paths(&target),
        ["/items/1", "/items/2"],
        "traffic untouched"
    );
    assert!(lib.open(&saved.id, &target, OpenMode::Append).is_err());
    assert_eq!(target.stats().exchanges, 2, "nothing half-inserted");
}
