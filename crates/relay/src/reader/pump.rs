//! A window's pump: one task per window that asks the server for the window's bytes and appends them as the limits allow, asking
//! again with `If-Range` when the body breaks off (ADR 0026, "Errors").

use std::sync::Arc;

use current_net::header::{CONTENT_RANGE, HeaderMap, HeaderValue, IF_RANGE, RANGE};
use current_net::{GatewayShape, StreamError};
use hyper::body::Bytes;

use super::state::{Fetch, WindowStatus};
use super::{Inner, RouteCore, lock};
use crate::events::RelayEvent;
use crate::serve::{event_for, is_html, is_part_asked};
use crate::upstream::{StreamMethod, UpstreamBody, UpstreamRequest, UpstreamResponse};

/// Why a request gave no body to read.
pub(super) enum Failure {
    /// Worth asking again: the server could not be reached, did not answer in time, or the secure connection failed.
    Transient,
    /// Not worth asking again; the event says why (none for a 416, the player's own business).
    Final(Option<RelayEvent>),
}

impl Failure {
    pub(super) fn event(&self) -> Option<RelayEvent> {
        match self {
            Self::Transient => Some(RelayEvent::Network),
            Self::Final(event) => event.clone(),
        }
    }
}

/// Asks for the file from `from` to its end. `validator` goes as `If-Range`: a server whose file changed answers 200 with the
/// whole new file, which is refused at any offset. Without a validator only the first request may be answered 200. `size`, once
/// known, must be the total of every 206's `Content-Range`: another length is another file.
pub(super) async fn request(
    core: &RouteCore,
    path: &str,
    query: &[(String, String)],
    from: u64,
    validator: Option<&HeaderValue>,
    size: Option<u64>,
) -> Result<UpstreamResponse, Failure> {
    let range = format!("bytes={from}-");
    let mut headers = HeaderMap::new();
    // The value is ASCII digits and punctuation: it always makes a header value.
    if let Ok(value) = HeaderValue::from_str(&range) {
        headers.insert(RANGE, value);
    }
    if let Some(validator) = validator {
        headers.insert(IF_RANGE, validator.clone());
    }
    let request = UpstreamRequest {
        method: StreamMethod::Get,
        path: path.to_owned(),
        query: query.to_vec(),
        headers,
    };
    let response = core
        .upstream
        .send(request)
        .await
        .map_err(|error| match error {
            StreamError::Unreachable | StreamError::Timeout | StreamError::Tls => {
                Failure::Transient
            }
            error => Failure::Final(event_for(&error)),
        })?;
    let status = response.status;
    if (200..300).contains(&status) && is_html(&response.headers) {
        return Err(Failure::Final(Some(RelayEvent::Refused {
            status,
            shape: GatewayShape::SignIn,
        })));
    }
    match status {
        206 if is_part_asked(Some(&range), &response.headers)
            && size.is_none_or(|size| total_of(&response.headers) == Some(size)) =>
        {
            Ok(response)
        }
        200 if from == 0 && validator.is_none() && size.is_none() => Ok(response),
        // A 200 to If-Range or for a later range (the file changed, or ranges are ignored), another part than the one asked for,
        // another total length, or a range the file no longer has.
        200 | 206 | 416 => Err(Failure::Final(Some(RelayEvent::BadRange))),
        status => Err(Failure::Final(Some(RelayEvent::Server { status }))),
    }
}

/// The total length in a `Content-Range` (`bytes <first>-<last>/<total>`).
fn total_of(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(CONTENT_RANGE)?
        .to_str()
        .ok()?
        .rsplit_once('/')?
        .1
        .trim()
        .parse()
        .ok()
}

/// Fetches the window `id` while it exists. `body` is the first answer's when the reader was just opened.
pub(super) async fn run(inner: Arc<Inner>, id: u64, mut body: Option<Box<dyn UpstreamBody>>) {
    // A chunk read past what the limits let in, appended once they do.
    let mut pending: Option<Bytes> = None;
    let mut tries = 0u32;
    loop {
        let (from, room) = inner
            .wait_until(|state| match state.fetch(id) {
                Fetch::Stop => Some(None),
                Fetch::Wait => None,
                Fetch::Room { from, room } => Some(Some((from, room))),
            })
            .await
            .unwrap_or((0, 0));
        if room == 0 {
            return;
        }
        if let Some(mut bytes) = pending.take() {
            if bytes.len() as u64 > room {
                // `room` is less than the chunk's length, a usize.
                pending = Some(bytes.split_off(room as usize));
            }
            if !lock(&inner.state).append(id, bytes) {
                return;
            }
            inner.changed.notify_waiters();
            continue;
        }
        let Some(current) = body.as_mut() else {
            lock(&inner.state).requests += 1;
            match request(
                &inner.core,
                &inner.path,
                &inner.query,
                from,
                inner.validator.as_ref(),
                Some(inner.size),
            )
            .await
            {
                Ok(response) => body = Some(response.body),
                Err(Failure::Transient) => {
                    if !retry(&inner, &mut tries).await {
                        fail(&inner, id, Some(RelayEvent::Network));
                        return;
                    }
                }
                Err(Failure::Final(event)) => {
                    fail(&inner, id, event);
                    return;
                }
            }
            continue;
        };
        match current.chunk().await {
            Ok(Some(chunk)) => {
                tries = 0;
                pending = Some(chunk);
            }
            // The body ended before the file did, or broke off: ask again from the next byte.
            Ok(None) | Err(_) => {
                body = None;
                if !retry(&inner, &mut tries).await {
                    fail(&inner, id, Some(RelayEvent::Network));
                    return;
                }
            }
        }
    }
}

/// Counts a try and waits before it; false when the tries are used up.
async fn retry(inner: &Inner, tries: &mut u32) -> bool {
    let backoff = {
        let mut state = lock(&inner.state);
        if *tries >= state.limits.tries {
            return false;
        }
        state.reconnects += 1;
        state.limits.backoff
    };
    let wait = backoff.saturating_mul(1 << (*tries).min(16));
    *tries += 1;
    if !wait.is_zero() {
        tokio::time::sleep(wait).await;
    }
    true
}

fn fail(inner: &Inner, id: u64, event: Option<RelayEvent>) {
    if let Some(event) = event {
        inner.report(event);
    }
    if let Some(window) = lock(&inner.state).window_mut(id) {
        window.status = WindowStatus::Failed;
    }
    inner.changed.notify_waiters();
}
