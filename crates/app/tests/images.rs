//! Images end to end through `App` (ADR 0018, task 2), against the Jellyfin stand-in on 127.0.0.1: the gateway's header reaches the
//! image endpoint, a cached image is served without a request (from memory, then from disk after a restart), signing out of and
//! removing a server delete its folder, the start deletes folders of servers no longer signed in, cancelling stops a fetch only
//! when no caller waits for it, file names hold only hashes, and no address, server or user name reaches a log line or a file
//! name. Settings and the cache go to a temporary folder, secrets to memory.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use current_app::{
    AddServerState, App, HeaderInput, ImageError, ImageLoad, ImageSource, SignInState,
};
use current_jellyfin::AuthError;
use current_model::{ImageRef, ImageType, ItemId, ItemRef, ServerKey, ServerRef, ServiceType};
use current_store::{MemorySecrets, Secrets};
use jellyfin_stand_in::{
    GATE_HEADER, GATE_HEADER_VALUE, Gate, IMAGE, PASSWORD, SERVER_NAME, StandIn, USER_NAME,
};

const ITEM: &str = "57a2d1e0000000000000000000000010";
const OTHER_ITEM: &str = "57a2d1e0000000000000000000000011";
const TAG: &str = "7a9e00000000000000000000000000aa";

fn jellyfin(key: &str) -> ServerRef {
    ServerRef::new(ServiceType::JELLYFIN, ServerKey::new(key))
}

fn open(dir: &Path, secrets: &Arc<MemorySecrets>) -> App {
    let secrets: Arc<dyn Secrets> = secrets.clone();
    App::open(&dir.join("data"), &dir.join("cache"), secrets, "Office PC")
        .unwrap()
        .0
}

fn poster(item: &str, width: u32, height: u32) -> ImageLoad {
    ImageLoad {
        source: ImageSource::Server(ImageRef {
            item: ItemRef::new(jellyfin("1"), ItemId::new(item)),
            kind: ImageType::PRIMARY,
            index: 0,
            tag: TAG.into(),
        }),
        width,
        height,
    }
}

fn path(item: &str) -> String {
    format!("/Items/{item}/Images/Primary")
}

/// Waits (at most 5 s) until `check` passes.
async fn eventually(mut check: impl FnMut() -> bool) {
    for _ in 0..500 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition not met within 5 s");
}

/// Every file and folder name under `dir`, recursively.
fn names(dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            found.push(entry.file_name().into_string().unwrap());
            if entry.file_type().unwrap().is_dir() {
                found.extend(names(&entry.path()));
            }
        }
    }
    found
}

fn image_files(dir: &Path) -> usize {
    names(&dir.join("cache").join("images"))
        .iter()
        .filter(|name| name.len() == 32)
        .count()
        .saturating_sub(server_folders(dir))
}

fn server_folders(dir: &Path) -> usize {
    std::fs::read_dir(dir.join("cache").join("images"))
        .map(|entries| entries.count())
        .unwrap_or(0)
}

/// Sets up and signs in to the stand-in behind `gate` (its header given when the gate asks), and waits until the session has an
/// address.
async fn signed_in(stand_in: &StandIn, app: &App) {
    let setup = app.add_server();
    setup.probe(stand_in.address()).await;
    if matches!(
        *setup.state().borrow(),
        AddServerState::GatewayRefused { .. }
    ) {
        setup
            .submit_headers(
                &[HeaderInput {
                    name: GATE_HEADER.into(),
                    value: GATE_HEADER_VALUE.into(),
                }],
                false,
            )
            .await
            .unwrap();
    }
    let found = match setup.state().borrow().clone() {
        AddServerState::Ready { server } => server,
        state => panic!("{state:?}"),
    };
    let flow = app.sign_in(found).unwrap();
    flow.with_password(USER_NAME, PASSWORD).await;
    let state = flow.state().borrow().clone();
    assert!(matches!(state, SignInState::SignedIn { .. }), "{state:?}");
    eventually(|| app.capabilities().borrow().servers[0].address.is_some()).await;
}

#[tokio::test]
async fn through_the_gateway_once_then_from_memory_then_from_disk_after_a_restart() {
    let stand_in = StandIn::start().await.unwrap();
    stand_in.set_gate(Gate::Header);
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    signed_in(&stand_in, &app).await;

    // 300 by 450 physical pixels: fetched as 320 by 480.
    let bytes = app.load_image(&poster(ITEM, 300, 450)).await.unwrap();
    assert_eq!(&*bytes, IMAGE);
    let seen = stand_in.images();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].gate_header, GATE_HEADER_VALUE,
        "the gateway's header reached the image endpoint"
    );
    assert_eq!(seen[0].accept, "image/jpeg, image/png");
    assert_eq!(
        seen[0].target,
        format!(
            "{}?fillWidth=320&fillHeight=480&quality=90&tag={TAG}",
            path(ITEM)
        )
    );

    // Another box in the same bucket: from memory, no request.
    let again = app.load_image(&poster(ITEM, 310, 465)).await.unwrap();
    assert_eq!(&*again, IMAGE);
    assert_eq!(stand_in.images().len(), 1);

    // The disk write is not awaited by the load.
    eventually(|| image_files(temp.path()) == 1).await;
    drop(app);
    let app = open(temp.path(), &secrets);
    let from_disk = app.load_image(&poster(ITEM, 300, 450)).await.unwrap();
    assert_eq!(&*from_disk, IMAGE);
    assert_eq!(
        stand_in.images().len(),
        1,
        "served from disk without a request"
    );

    // Names under the cache folder are hashes only; nothing names the server, its address, the user, the item or the tag.
    let port = stand_in.address().rsplit(':').next().unwrap().to_owned();
    let all = names(&temp.path().join("cache"));
    assert!(all.contains(&"images".to_owned()));
    for name in all.iter().filter(|name| *name != "images") {
        assert!(
            name.len() == 32 && name.bytes().all(|b| b.is_ascii_hexdigit()),
            "{name}"
        );
    }
    let text = format!("{}\n{}", all.join("\n"), app.diagnostics_text());
    for private in [
        &port,
        "127.0.0.1",
        USER_NAME,
        SERVER_NAME,
        TAG,
        GATE_HEADER_VALUE,
    ] {
        assert!(!text.contains(private), "{private} in:\n{text}");
    }
}

#[tokio::test]
async fn signing_out_deletes_the_servers_images() {
    let stand_in = StandIn::start().await.unwrap();
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    signed_in(&stand_in, &app).await;
    app.load_image(&poster(ITEM, 320, 480)).await.unwrap();
    eventually(|| image_files(temp.path()) == 1).await;

    app.sign_out(&jellyfin("1")).await.unwrap();
    assert_eq!(server_folders(temp.path()), 0);
    // Gone from memory too: without a session it cannot be fetched again.
    assert_eq!(
        app.load_image(&poster(ITEM, 320, 480)).await,
        Err(ImageError::Fetch(current_jellyfin::ImageError::Request(
            AuthError::NotSignedIn
        )))
    );
}

#[tokio::test]
async fn removing_a_server_deletes_its_images() {
    let stand_in = StandIn::start().await.unwrap();
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    signed_in(&stand_in, &app).await;
    app.load_image(&poster(ITEM, 320, 480)).await.unwrap();
    eventually(|| image_files(temp.path()) == 1).await;

    app.remove_server(&jellyfin("1")).await.unwrap();
    assert_eq!(server_folders(temp.path()), 0);
}

#[tokio::test]
async fn the_start_deletes_folders_of_servers_not_signed_in() {
    let stand_in = StandIn::start().await.unwrap();
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    signed_in(&stand_in, &app).await;
    app.load_image(&poster(ITEM, 320, 480)).await.unwrap();
    eventually(|| image_files(temp.path()) == 1).await;
    drop(app);
    // A server removed while the app could not delete its folder, say.
    let stray = temp
        .path()
        .join("cache")
        .join("images")
        .join("0123456789abcdef0123456789abcdef");
    std::fs::create_dir_all(&stray).unwrap();
    std::fs::write(stray.join("fedcba9876543210fedcba9876543210"), b"old").unwrap();

    let app = open(temp.path(), &secrets);
    assert!(!stray.exists());
    assert_eq!(
        image_files(temp.path()),
        1,
        "the signed-in server's images stay"
    );
    drop(app);
}

#[tokio::test]
async fn a_shared_fetch_goes_on_while_one_caller_waits_and_stops_when_none_does() {
    let stand_in = StandIn::start().await.unwrap();
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = Arc::new(open(temp.path(), &secrets));
    signed_in(&stand_in, &app).await;
    let load = |item: &'static str| {
        let app = app.clone();
        tokio::spawn(async move { app.load_image(&poster(item, 320, 480)).await })
    };

    // Two callers, one request; the first gives up, the second still gets the image.
    stand_in.hold(&path(ITEM));
    let gives_up = load(ITEM);
    let waits = load(ITEM);
    eventually(|| stand_in.count(&path(ITEM)) == 1).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        stand_in.count(&path(ITEM)),
        1,
        "identical requests share one fetch"
    );
    gives_up.abort();
    tokio::time::sleep(Duration::from_millis(50)).await;
    stand_in.release(&path(ITEM));
    assert_eq!(&*waits.await.unwrap().unwrap(), IMAGE);
    assert_eq!(stand_in.cancelled(&path(ITEM)), 0);

    // Both give up: the request is dropped, which the stand-in sees as a closed connection.
    stand_in.hold(&path(OTHER_ITEM));
    let first = load(OTHER_ITEM);
    let second = load(OTHER_ITEM);
    eventually(|| stand_in.count(&path(OTHER_ITEM)) == 1).await;
    first.abort();
    second.abort();
    eventually(|| stand_in.cancelled(&path(OTHER_ITEM)) == 1).await;
    stand_in.release(&path(OTHER_ITEM));
    assert!(
        stand_in
            .images()
            .iter()
            .all(|seen| !seen.target.contains(OTHER_ITEM))
    );
}

#[tokio::test]
async fn placeholders_are_bmps_and_bad_requests_fail_without_a_request() {
    let app = App::new();
    let placeholder = ImageLoad {
        source: ImageSource::BlurHash("LEHV6nWB2yk8pyo0adR*.7kCMdnj".into()),
        width: 640,
        height: 360,
    };
    let bmp = app.load_image(&placeholder).await.unwrap();
    assert_eq!(&bmp[..2], b"BM");
    let invalid = ImageLoad {
        source: ImageSource::BlurHash("???".into()),
        ..placeholder.clone()
    };
    assert_eq!(
        app.load_image(&invalid).await,
        Err(ImageError::InvalidBlurHash)
    );
    let empty = ImageLoad {
        width: 0,
        ..placeholder
    };
    assert_eq!(app.load_image(&empty).await, Err(ImageError::EmptyBox));
}
