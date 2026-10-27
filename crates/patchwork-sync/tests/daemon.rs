//! The daemon's side of things that doesn't need the network: answering,
//! pausing and stopping.

use std::time::Duration;

use patchwork_core::Patchwork;
use patchwork_core::paths::Paths;
use patchwork_sync::daemon::{self, State};

#[tokio::test(flavor = "multi_thread")]
async fn the_daemon_pauses_for_others_and_stops_when_told() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(tmp.path());
    let dir = paths.sync();
    assert!(daemon::status(&dir).unwrap().is_none());

    let opening = paths.clone();
    let serving = tokio::spawn({
        let dir = dir.clone();
        async move { daemon::serve(&dir, || Patchwork::builder(opening.clone()).open()).await }
    });
    let status = loop {
        if let Some(status) = tokio::task::block_in_place(|| daemon::status(&dir)).unwrap() {
            break status;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(status.version, env!("CARGO_PKG_VERSION"));

    // nothing's synced, so there's nothing to do until something is
    let paused = tokio::task::block_in_place(|| daemon::pause(&dir)).unwrap();
    assert!(paused.is_some());
    // it may have been idle, which it can be paused from straight away
    let status = loop {
        let status = tokio::task::block_in_place(|| daemon::status(&dir))
            .unwrap()
            .unwrap();
        if status.state != State::Idle && status.state != State::Starting {
            break status;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(status.paused_by, 1);
    assert_eq!(status.state, State::Paused);
    // and patchwork can be opened meanwhile
    drop(Patchwork::builder(paths.clone()).open().unwrap());
    drop(paused);

    assert!(tokio::task::block_in_place(|| daemon::stop(&dir)).unwrap());
    tokio::time::timeout(Duration::from_secs(10), serving)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(daemon::status(&dir).unwrap().is_none());
}
