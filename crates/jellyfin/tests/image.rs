//! Fetching images against a hand-written stand-in on 127.0.0.1 (ADR 0018, section 4): the request line and `Accept` header on the
//! wire, the bytes back, and the answers that are not images: a gateway's HTML page, a JSON body, a 404.

use std::sync::{Arc, Mutex};

use current_jellyfin::{
    AccessToken, AuthError, ClientIdentity, GatewayAnswer, ImageError, ImageRequest, JellyfinClient,
};
use current_model::{ImageType, ItemId, ItemRef, ServerKey, ServerRef, ServiceType};
use current_net::{FixedAddress, NetError, RequestLog, Url};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const TOKEN: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90";
const ITEM: &str = "0123456789abcdef0123456789abcdef";
/// The 8-byte PNG signature and a few bytes after it: enough for the client, which does not decode.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR";

/// What the stand-in answers: status, content type and body.
type Answer = (u16, Option<&'static str>, Vec<u8>);

/// A server that answers every request with `answer` and keeps each request's head.
async fn stand_in(answer: Answer) -> (Url, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = Url::parse(&format!(
        "http://127.0.0.1:{}",
        listener.local_addr().unwrap().port()
    ))
    .unwrap();
    let heads = Arc::new(Mutex::new(Vec::new()));
    let kept = heads.clone();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut data = Vec::new();
            let mut buffer = [0u8; 4096];
            while !data.windows(4).any(|w| w == b"\r\n\r\n") {
                match socket.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(read) => data.extend_from_slice(&buffer[..read]),
                }
            }
            kept.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&data).into_owned());
            let (status, content_type, body) = &answer;
            let mut head = format!(
                "HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n",
                body.len()
            );
            if let Some(content_type) = content_type {
                head.push_str(&format!("content-type: {content_type}\r\n"));
            }
            head.push_str("\r\n");
            let _ = socket.write_all(head.as_bytes()).await;
            let _ = socket.write_all(body).await;
            let _ = socket.shutdown().await;
        }
    });
    (url, heads)
}

fn client(url: Url) -> JellyfinClient {
    let server = ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1"));
    let client = JellyfinClient::new(
        server,
        Arc::new(FixedAddress(url)),
        ClientIdentity::current("Example PC", "device-1"),
        Arc::new(RequestLog::default()),
    )
    .unwrap();
    client.set_token(AccessToken::new(TOKEN));
    client
}

fn request() -> ImageRequest {
    ImageRequest {
        item: ItemRef::new(
            ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")),
            ItemId::new(ITEM),
        ),
        kind: ImageType::BACKDROP,
        index: 1,
        tag: "7a9e0000tag".into(),
        width: 640,
        height: 360,
    }
}

#[tokio::test]
async fn an_image_comes_back_as_its_bytes_from_the_url_it_was_asked_for() {
    let (url, heads) = stand_in((200, Some("image/png"), PNG.to_vec())).await;
    let bytes = client(url).image(&request()).await.unwrap();
    assert_eq!(bytes, PNG);
    let head = heads.lock().unwrap()[0].clone();
    let line = head.lines().next().unwrap();
    assert_eq!(
        line,
        format!(
            "GET /Items/{ITEM}/Images/Backdrop/1?fillWidth=640&fillHeight=360&quality=90&tag=7a9e0000tag HTTP/1.1"
        )
    );
    assert!(
        head.lines()
            .any(|header| header.eq_ignore_ascii_case("accept: image/jpeg, image/png")),
        "{head}"
    );
    assert!(
        head.contains(TOKEN),
        "the session's token goes with the request"
    );
}

#[tokio::test]
async fn a_jpeg_without_a_content_type_is_still_an_image() {
    let jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 16];
    let (url, _) = stand_in((200, None, jpeg.clone())).await;
    assert_eq!(client(url).image(&request()).await.unwrap(), jpeg);
}

#[tokio::test]
async fn a_gateways_page_is_the_gateways_answer() {
    let page = b"<!DOCTYPE html><html><body>Sign in</body></html>".to_vec();
    let (url, _) = stand_in((200, Some("text/html"), page)).await;
    assert_eq!(
        client(url).image(&request()).await,
        Err(ImageError::Request(AuthError::Gateway(
            GatewayAnswer::SignIn
        )))
    );
}

#[tokio::test]
async fn json_is_not_an_image() {
    let (url, _) = stand_in((200, Some("application/json"), b"{}".to_vec())).await;
    assert_eq!(
        client(url).image(&request()).await,
        Err(ImageError::NotAnImage)
    );
}

#[tokio::test]
async fn a_missing_image_is_the_servers_status() {
    let (url, _) = stand_in((404, None, Vec::new())).await;
    assert!(matches!(
        client(url).image(&request()).await,
        Err(ImageError::Request(AuthError::Network(NetError::Status {
            status: 404,
            ..
        })))
    ));
}
