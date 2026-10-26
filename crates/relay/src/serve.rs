//! One request: the refusals that come before anything goes upstream (ADR 0020, "Routes and who may use it"), then forwarding, the
//! answer's headers, playlists and events ("Requests and answers", "HLS playlists", "Lifecycle").

use std::sync::Arc;

use current_net::header::{
    ACCEPT_RANGES, CONNECTION, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, ETAG, HOST, HeaderMap,
    HeaderValue, ORIGIN, RANGE, TRANSFER_ENCODING,
};
use current_net::{GatewayShape, STREAM_REQUEST_HEADERS, STREAM_RESPONSE_HEADERS, StreamError};
use hyper::body::{Body, Bytes, Incoming};
use hyper::{Method, Request, Response, StatusCode};

use crate::body::RelayBody;
use crate::events::RelayEvent;
use crate::playlist::{
    MAX_PLAYLIST_BYTES, PlaylistError, Rewriter, is_api_key, is_playlist, starts_like_playlist,
};
use crate::relay::{Bind, Conn, RouteEntry};
use crate::upstream::{StreamMethod, UpstreamBody, UpstreamRequest, UpstreamResponse};

pub(crate) async fn handle(request: Request<Incoming>, conn: &Conn) -> Response<RelayBody> {
    conn.saw_request();
    match admit(&request, conn) {
        Ok((route, upstream)) => forward(route, upstream).await,
        Err(status) => refused(status),
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
    if !route.prefixes.allow(rest) {
        return Err(StatusCode::NOT_FOUND);
    }
    match conn.bind(&route) {
        Bind::Bound => {}
        Bind::Removed => return Err(StatusCode::NOT_FOUND),
        // A connection serves one route: one that asks for another is not a player's, and is closed rather than answered.
        Bind::Other => {
            conn.close();
            return Err(StatusCode::NOT_FOUND);
        }
    }
    let query = match uri.query() {
        Some(query) => decode_query(query).ok_or(StatusCode::BAD_REQUEST)?,
        None => Vec::new(),
    };
    let mut headers = HeaderMap::new();
    for name in &STREAM_REQUEST_HEADERS {
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

/// A query's pairs, decoded (`application/x-www-form-urlencoded`), without `ApiKey` and `api_key`. `None` for a `%` not followed
/// by two hex digits or a value that does not decode to UTF-8: refused rather than passed on altered.
fn decode_query(query: &str) -> Option<Vec<(String, String)>> {
    let mut pairs = Vec::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let (key, value) = (decode_component(key)?, decode_component(value)?);
        if !is_api_key(&key) {
            pairs.push((key, value));
        }
    }
    Some(pairs)
}

fn decode_component(text: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(text.len());
    let mut input = text.bytes();
    while let Some(byte) = input.next() {
        match byte {
            b'+' => bytes.push(b' '),
            b'%' => {
                let high = char::from(input.next()?).to_digit(16)?;
                let low = char::from(input.next()?).to_digit(16)?;
                // Two hex digits make a byte: the cast cannot truncate.
                bytes.push((high * 16 + low) as u8);
            }
            byte => bytes.push(byte),
        }
    }
    String::from_utf8(bytes).ok()
}

async fn forward(route: Arc<RouteEntry>, request: UpstreamRequest) -> Response<RelayBody> {
    let method = request.method;
    let path = request.path.clone();
    let range = request
        .headers
        .get(RANGE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let response = match route.upstream.send(request).await {
        Ok(response) => response,
        Err(error) => return failed(&route, &error),
    };
    let status = response.status;
    let success = (200..300).contains(&status);
    // An upstream outside its contract ([`crate::Upstream`]): nothing of its answer is passed on.
    if !success && status != 304 && status != 416 {
        route.events.report(RelayEvent::Server { status });
        return empty(StatusCode::BAD_GATEWAY);
    }
    // A part other than the one asked for: the sniffing below and the player would both misjudge where the body starts.
    if status == 206 && !is_part_asked(range.as_deref(), &response.headers) {
        route.events.report(RelayEvent::BadRange);
        return empty(StatusCode::BAD_GATEWAY);
    }
    if success && is_html(&response.headers) {
        route.events.report(RelayEvent::Refused {
            status,
            shape: GatewayShape::SignIn,
        });
        return empty(StatusCode::BAD_GATEWAY);
    }
    // The route's HLS entry is a playlist whatever it says: anything else there fails to rewrite.
    let entry = route.hls_entry.as_deref() == Some(path.as_str());
    let declared = success && (entry || is_playlist(&response.headers, &path));
    if method == StreamMethod::Head || !success {
        // A HEAD of a playlist gets no length: the rewritten one is not known without the body. A 304 and a 416 have none to give.
        let mut answer = Response::new(RelayBody::Empty);
        *answer.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
        copy_headers(&response.headers, answer.headers_mut(), declared);
        if !success {
            answer.headers_mut().remove(CONTENT_LENGTH);
        }
        return answer;
    }
    let UpstreamResponse {
        headers,
        base,
        mut body,
        ..
    } = response;
    // A player may take any body that starts like a playlist for one, whatever its type or path: those are rewritten too.
    let mut head: Vec<Bytes> = Vec::new();
    let mut sniffed = Vec::new();
    let mut done = false;
    // Only a body from its first byte: a seek into a media file can land anywhere, whitespace included.
    let playlist = declared
        || from_start(&headers)
            && loop {
                if let Some(playlist) = starts_like_playlist(&sniffed, done) {
                    break playlist;
                }
                match body.chunk().await {
                    Ok(Some(chunk)) => {
                        sniffed.extend_from_slice(&chunk);
                        head.push(chunk);
                    }
                    Ok(None) => done = true,
                    Err(_) => {
                        route.events.report(RelayEvent::Network);
                        return empty(StatusCode::BAD_GATEWAY);
                    }
                }
            };
    drop(sniffed);
    if playlist {
        let rest = (!done).then_some(body);
        return rewrite_playlist(&route, &path, status, &headers, &base, head, rest).await;
    }
    let mut answer = Response::new(RelayBody::stream(
        head,
        (!done).then_some(body),
        route.events.clone(),
    ));
    *answer.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    copy_headers(&headers, answer.headers_mut(), false);
    answer
}

/// A playlist, read whole and rewritten. A 206 is passed on only when it holds the whole playlist (FFmpeg asks for `bytes=0-`), as
/// a 200; a part of one cannot be rewritten.
async fn rewrite_playlist(
    route: &RouteEntry,
    path: &str,
    status: u16,
    headers: &HeaderMap,
    base: &current_net::Url,
    head: Vec<Bytes>,
    rest: Option<Box<dyn UpstreamBody>>,
) -> Response<RelayBody> {
    let text = match read_playlist(head, rest).await {
        Ok(text) => text,
        Err(event) => {
            route.events.report(event);
            return empty(StatusCode::BAD_GATEWAY);
        }
    };
    if status == 206 && !is_whole(headers, text.len()) {
        route.events.report(RelayEvent::PlaylistUnreadable);
        return empty(StatusCode::BAD_GATEWAY);
    }
    let mut location = base.clone();
    location.set_path(&format!("{}{path}", base.path().trim_end_matches('/')));
    location.set_query(None);
    let rewriter = Rewriter {
        playlist: &location,
        base,
        prefixes: &route.prefixes,
        token: route.token.as_str(),
    };
    let rewritten = match rewriter.rewrite(&text) {
        Ok(rewritten) => rewritten,
        Err(error) => {
            route.events.report(match error {
                PlaylistError::Elsewhere => RelayEvent::PlaylistElsewhere,
                PlaylistError::Malformed | PlaylistError::TooLarge => {
                    RelayEvent::PlaylistUnreadable
                }
            });
            return empty(StatusCode::BAD_GATEWAY);
        }
    };
    let length = rewritten.len();
    let mut answer = Response::new(RelayBody::Full(Some(Bytes::from(rewritten))));
    *answer.status_mut() = if status == 206 {
        StatusCode::OK
    } else {
        StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY)
    };
    copy_headers(headers, answer.headers_mut(), true);
    answer
        .headers_mut()
        .insert(CONTENT_LENGTH, HeaderValue::from(length));
    answer
}

/// Whether a 206's `Content-Range` is the part `range` (the request's `Range`) asked for: a single range (`bytes=<first>-[<last>]`,
/// or `bytes=-<suffix>` with a known length) whose answer starts where it asked. A 206 to a request without `Range`, or without a
/// readable `Content-Range`, is not.
fn is_part_asked(range: Option<&str>, headers: &HeaderMap) -> bool {
    let Some(asked) = range.and_then(|range| range.trim().strip_prefix("bytes=")) else {
        return false;
    };
    let Some((start, total)) = headers
        .get(CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().strip_prefix("bytes "))
        .and_then(|value| value.split_once('/'))
        .and_then(|(span, total)| {
            let (start, _) = span.split_once('-')?;
            Some((
                start.trim().parse::<u64>().ok()?,
                total.trim().parse::<u64>().ok(),
            ))
        })
    else {
        return false;
    };
    if asked.contains(',') {
        return false;
    }
    let Some((first, _)) = asked.split_once('-') else {
        return false;
    };
    let first = first.trim();
    if first.is_empty() {
        // A suffix: the last <suffix> bytes.
        let suffix = asked
            .split_once('-')
            .and_then(|(_, suffix)| suffix.trim().parse::<u64>().ok());
        return match (suffix, total) {
            (Some(suffix), Some(total)) => start == total.saturating_sub(suffix),
            _ => false,
        };
    }
    first.parse::<u64>().is_ok_and(|first| first == start)
}

/// Whether an answer's body starts at the resource's first byte: no `Content-Range`, or one from 0.
fn from_start(headers: &HeaderMap) -> bool {
    headers.get(CONTENT_RANGE).is_none_or(|range| {
        range
            .to_str()
            .is_ok_and(|range| range.starts_with("bytes 0-"))
    })
}

/// Whether a 206's `Content-Range` (`bytes 0-<end>/<total>`) covers the whole resource, `length` bytes long.
fn is_whole(headers: &HeaderMap, length: usize) -> bool {
    let Some(range) = headers
        .get(CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let whole = range
        .strip_prefix("bytes 0-")
        .and_then(|rest| rest.split_once('/'))
        .and_then(|(end, total)| Some((end.parse::<usize>().ok()?, total.parse::<usize>().ok()?)));
    whole.is_some_and(|(end, total)| end + 1 == total && total == length)
}

async fn read_playlist(
    head: Vec<Bytes>,
    rest: Option<Box<dyn UpstreamBody>>,
) -> Result<String, RelayEvent> {
    let mut text = Vec::new();
    for chunk in head {
        text.extend_from_slice(&chunk);
    }
    if text.len() > MAX_PLAYLIST_BYTES {
        return Err(RelayEvent::PlaylistUnreadable);
    }
    if let Some(mut body) = rest {
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
    }
    String::from_utf8(text).map_err(|_| RelayEvent::PlaylistUnreadable)
}

/// The answer to a failed upstream request, after reporting it. The player sees the server's status only where it means something
/// to a player (404, 416 and 5xx); every other failure is a 502.
fn failed(route: &RouteEntry, error: &StreamError) -> Response<RelayBody> {
    let status = match error {
        StreamError::Refused { status, shape } => {
            route.events.report(RelayEvent::Refused {
                status: *status,
                shape: shape.clone(),
            });
            StatusCode::BAD_GATEWAY
        }
        StreamError::Status { status: 404 } => {
            route.events.report(RelayEvent::NotFound);
            StatusCode::NOT_FOUND
        }
        // A range past the end: the player's own business (it probes the end of a file), not a failure of the stream.
        StreamError::Status { status: 416 } => StatusCode::RANGE_NOT_SATISFIABLE,
        StreamError::Status { status } => {
            route.events.report(RelayEvent::Server { status: *status });
            StatusCode::from_u16(*status)
                .ok()
                .filter(StatusCode::is_server_error)
                .unwrap_or(StatusCode::BAD_GATEWAY)
        }
        StreamError::Unreachable | StreamError::Tls => {
            route.events.report(RelayEvent::Network);
            StatusCode::BAD_GATEWAY
        }
        StreamError::Timeout => {
            route.events.report(RelayEvent::Network);
            StatusCode::GATEWAY_TIMEOUT
        }
        StreamError::InvalidRequest => {
            route.events.report(RelayEvent::InvalidRequest);
            StatusCode::BAD_GATEWAY
        }
    };
    empty(status)
}

/// Whether an answer says it is an HTML page: a gateway's sign-in where media was expected. Only the declared type: media bodies
/// are not sniffed for it.
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

/// Copies [`STREAM_RESPONSE_HEADERS`]. For a playlist, without the length, entity tag and ranges of the text before rewriting.
fn copy_headers(from: &HeaderMap, to: &mut HeaderMap, playlist: bool) {
    for name in &STREAM_RESPONSE_HEADERS {
        if playlist
            && (name == CONTENT_LENGTH
                || name == ETAG
                || name == CONTENT_RANGE
                || name == ACCEPT_RANGES)
        {
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

/// A refusal, which also closes the connection: a client that sends one is not a player, and must not keep a connection slot.
fn refused(status: StatusCode) -> Response<RelayBody> {
    let mut response = empty(status);
    response
        .headers_mut()
        .insert(CONNECTION, HeaderValue::from_static("close"));
    response
}
