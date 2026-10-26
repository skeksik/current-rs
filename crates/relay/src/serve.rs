//! One request: the refusals that come before anything goes upstream (ADR 0020, "Routes and who may use it"), then forwarding, the
//! answer's headers, playlists and events ("Requests and answers", "HLS playlists", "Lifecycle").

use std::sync::Arc;

use current_net::header::{
    CONTENT_LENGTH, CONTENT_TYPE, ETAG, HOST, HeaderMap, HeaderValue, ORIGIN, TRANSFER_ENCODING,
};
use current_net::{GatewayShape, NetError, classify};
use hyper::body::{Body, Bytes, Incoming};
use hyper::{Method, Request, Response, StatusCode};

use crate::body::RelayBody;
use crate::events::RelayEvent;
use crate::playlist::{MAX_PLAYLIST_BYTES, PlaylistError, Rewriter, is_api_key, is_playlist};
use crate::relay::{Conn, RouteEntry};
use crate::upstream::{
    REQUEST_HEADERS, RESPONSE_HEADERS, StreamMethod, UpstreamBody, UpstreamRequest,
    UpstreamResponse,
};

pub(crate) async fn handle(request: Request<Incoming>, conn: &Conn) -> Response<RelayBody> {
    match admit(&request, conn) {
        Ok((route, upstream)) => forward(route, upstream).await,
        Err(status) => empty(status),
    }
}

/// The route and the upstream request for a player's request, or the status that refuses it. Nothing here goes upstream.
fn admit(
    request: &Request<Incoming>,
    conn: &Conn,
) -> Result<(Arc<RouteEntry>, UpstreamRequest), StatusCode> {
    if request.method() == Method::CONNECT {
        return Err(StatusCode::METHOD_NOT_ALLOWED);
    }
    // Only origin-form targets (`/...`): the host is never the request's to choose.
    let uri = request.uri();
    if uri.scheme().is_some() || uri.authority().is_some() || !uri.path().starts_with('/') {
        return Err(StatusCode::BAD_REQUEST);
    }
    // Against DNS rebinding: a page that reached the port under another name sends that name.
    let expected = format!("127.0.0.1:{}", conn.port());
    let mut hosts = request.headers().get_all(HOST).iter();
    match (hosts.next(), hosts.next()) {
        (Some(host), None) if host.as_bytes() == expected.as_bytes() => {}
        _ => return Err(StatusCode::BAD_REQUEST),
    }
    // Players send no Origin; browsers do on cross-origin requests.
    if request.headers().contains_key(ORIGIN) {
        return Err(StatusCode::FORBIDDEN);
    }
    let method = match *request.method() {
        Method::GET => StreamMethod::Get,
        Method::HEAD => StreamMethod::Head,
        _ => return Err(StatusCode::METHOD_NOT_ALLOWED),
    };
    if !request.body().is_end_stream() || request.headers().contains_key(TRANSFER_ENCODING) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let path = uri.path();
    let token = path[1..].split('/').next().unwrap_or("");
    let rest = &path[1 + token.len()..];
    let shared = conn.shared().ok_or(StatusCode::NOT_FOUND)?;
    // Every route's token is compared in full, whichever matches.
    let mut found = None;
    for route in shared.routes() {
        if route.token.matches(token) {
            found = Some(route);
        }
    }
    let route = found.ok_or(StatusCode::NOT_FOUND)?;
    if !route.prefixes.allow(rest) || !conn.bind(&route) {
        return Err(StatusCode::NOT_FOUND);
    }
    let query = uri
        .query()
        .map(|query| {
            url::form_urlencoded::parse(query.as_bytes())
                .filter(|(key, _)| !is_api_key(key))
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect()
        })
        .unwrap_or_default();
    let mut headers = HeaderMap::new();
    for name in &REQUEST_HEADERS {
        for value in request.headers().get_all(name) {
            headers.append(name.clone(), value.clone());
        }
    }
    let upstream = UpstreamRequest {
        method,
        path: rest.to_owned(),
        query,
        headers,
    };
    Ok((route, upstream))
}

async fn forward(route: Arc<RouteEntry>, request: UpstreamRequest) -> Response<RelayBody> {
    let method = request.method;
    let path = request.path.clone();
    let response = match route.upstream.send(request).await {
        Ok(response) => response,
        Err(error) => return failed(&route, &error),
    };
    let status = response.status;
    if !(200..300).contains(&status) && status != 304 {
        return failed(&route, &error_for(status, &response.headers));
    }
    let has_body = status != 304 && method == StreamMethod::Get;
    if status != 304 && is_html(&response.headers) {
        route.events.report(RelayEvent::Refused {
            shape: GatewayShape::SignIn,
        });
        return empty(StatusCode::BAD_GATEWAY);
    }
    if status != 304 && is_playlist(&response.headers, &path) {
        return playlist(&route, &path, response, has_body).await;
    }
    let mut answer = Response::new(if has_body {
        RelayBody::stream(response.body, route.events.clone())
    } else {
        RelayBody::Empty
    });
    *answer.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    copy_headers(&response.headers, answer.headers_mut(), false);
    answer
}

/// A playlist, read whole and rewritten. A HEAD gets the headers without a length, since the rewritten length is not known.
async fn playlist(
    route: &RouteEntry,
    path: &str,
    response: UpstreamResponse,
    has_body: bool,
) -> Response<RelayBody> {
    let UpstreamResponse {
        status,
        headers,
        base,
        body,
    } = response;
    let body = if has_body {
        let text = match read_playlist(body).await {
            Ok(text) => text,
            Err(event) => {
                route.events.report(event);
                return empty(StatusCode::BAD_GATEWAY);
            }
        };
        let mut location = base.clone();
        location.set_path(&format!("{}{path}", base.path().trim_end_matches('/')));
        location.set_query(None);
        let rewriter = Rewriter {
            playlist: &location,
            base: &base,
            prefixes: &route.prefixes,
            token: route.token.as_str(),
        };
        match rewriter.rewrite(&text) {
            Ok(rewritten) => RelayBody::Full(Some(Bytes::from(rewritten))),
            Err(error) => {
                route.events.report(match error {
                    PlaylistError::Elsewhere => RelayEvent::PlaylistElsewhere,
                    PlaylistError::Malformed => RelayEvent::PlaylistUnreadable,
                });
                return empty(StatusCode::BAD_GATEWAY);
            }
        }
    } else {
        RelayBody::Empty
    };
    let length = match &body {
        RelayBody::Full(Some(bytes)) => Some(bytes.len()),
        _ => None,
    };
    let mut answer = Response::new(body);
    *answer.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    // The upstream length and entity tag name the playlist before rewriting.
    copy_headers(&headers, answer.headers_mut(), true);
    if let Some(length) = length {
        answer
            .headers_mut()
            .insert(CONTENT_LENGTH, HeaderValue::from(length));
    }
    answer
}

async fn read_playlist(mut body: Box<dyn UpstreamBody>) -> Result<String, RelayEvent> {
    let mut text = Vec::new();
    loop {
        match body.chunk().await {
            Ok(Some(chunk)) => {
                if text.len() + chunk.len() > MAX_PLAYLIST_BYTES {
                    return Err(RelayEvent::PlaylistUnreadable);
                }
                text.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(_) => return Err(RelayEvent::Network),
        }
    }
    String::from_utf8(text).map_err(|_| RelayEvent::PlaylistUnreadable)
}

/// The answer to a failed upstream request, after reporting it.
fn failed(route: &RouteEntry, error: &NetError) -> Response<RelayBody> {
    let status = match error {
        NetError::Auth { .. } | NetError::Redirected { .. } => {
            route.events.report(RelayEvent::Refused {
                shape: classify(&Err(error.clone())),
            });
            StatusCode::BAD_GATEWAY
        }
        NetError::Status { status: 404, .. } => {
            route.events.report(RelayEvent::NotFound);
            StatusCode::NOT_FOUND
        }
        // A range past the end: the player's own business (it probes the end of a file), not a failure of the stream.
        NetError::Status { status: 416, .. } => StatusCode::RANGE_NOT_SATISFIABLE,
        NetError::Status { status, .. } => {
            route.events.report(RelayEvent::Server { status: *status });
            StatusCode::from_u16(*status)
                .ok()
                .filter(|status| status.is_client_error() || status.is_server_error())
                .unwrap_or(StatusCode::BAD_GATEWAY)
        }
        NetError::Unreachable | NetError::Tls => {
            route.events.report(RelayEvent::Network);
            StatusCode::BAD_GATEWAY
        }
        NetError::Timeout => {
            route.events.report(RelayEvent::Network);
            StatusCode::GATEWAY_TIMEOUT
        }
        // Not a server's answer: a request the upstream could not build, or a body it should not have decoded.
        _ => StatusCode::BAD_GATEWAY,
    };
    empty(status)
}

/// The error an `Ok` answer with this status should have been (the [`crate::Upstream`] contract), so it is handled the same.
fn error_for(status: u16, headers: &HeaderMap) -> NetError {
    match status {
        300..=399 => NetError::Redirected {
            status,
            location: None,
        },
        401 | 403 => NetError::Auth {
            status,
            message: None,
            challenges: Vec::new(),
            page: is_html(headers),
        },
        _ => NetError::Status {
            status,
            message: None,
        },
    }
}

/// Whether an answer says it is an HTML page: a gateway's sign-in where media was expected. Only the declared type: media bodies
/// are not sniffed.
fn is_html(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            let essence = value.split(';').next().unwrap_or("").trim();
            essence.eq_ignore_ascii_case("text/html")
                || essence.eq_ignore_ascii_case("application/xhtml+xml")
        })
}

fn copy_headers(from: &HeaderMap, to: &mut HeaderMap, playlist: bool) {
    for name in &RESPONSE_HEADERS {
        if playlist && (name == CONTENT_LENGTH || name == ETAG) {
            continue;
        }
        for value in from.get_all(name) {
            to.append(name.clone(), value.clone());
        }
    }
}

fn empty(status: StatusCode) -> Response<RelayBody> {
    let mut response = Response::new(RelayBody::Empty);
    *response.status_mut() = status;
    response
}
