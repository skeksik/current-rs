//! The body the relay sends a player: empty, a rewritten playlist, or the upstream body passed on one chunk at a time.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use hyper::body::{Body, Bytes, Frame};

use crate::events::{RelayEvent, RouteEvents};
use crate::upstream::{UpstreamBody, UpstreamError};

/// The upstream body broke off; the player's connection is closed. No detail: it could name the server.
#[derive(Debug, thiserror::Error)]
#[error("the server's answer broke off")]
pub(crate) struct BodyError;

type Next = Pin<
    Box<dyn Future<Output = (Box<dyn UpstreamBody>, Result<Option<Bytes>, UpstreamError>)> + Send>,
>;

pub(crate) enum RelayBody {
    Empty,
    Full(Option<Bytes>),
    Stream(Streaming),
}

/// An upstream body being passed on. hyper polls it only when the connection can write more, and it asks upstream for one chunk
/// per poll, so at most one chunk is held here (plus the few read to tell a playlist from media).
pub(crate) struct Streaming {
    queued: VecDeque<Bytes>,
    idle: Option<Box<dyn UpstreamBody>>,
    next: Option<Next>,
    events: Arc<RouteEvents>,
}

impl RelayBody {
    /// `queued` (chunks already read), then the rest of `body` if it has not ended.
    pub(crate) fn stream(
        queued: Vec<Bytes>,
        body: Option<Box<dyn UpstreamBody>>,
        events: Arc<RouteEvents>,
    ) -> Self {
        Self::Stream(Streaming {
            queued: queued.into(),
            idle: body,
            next: None,
            events,
        })
    }
}

impl Body for RelayBody {
    type Data = Bytes;
    type Error = BodyError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BodyError>>> {
        match self.get_mut() {
            Self::Empty => Poll::Ready(None),
            Self::Full(bytes) => Poll::Ready(bytes.take().map(|bytes| Ok(Frame::data(bytes)))),
            Self::Stream(streaming) => streaming.poll(cx),
        }
    }

    fn is_end_stream(&self) -> bool {
        match self {
            Self::Empty => true,
            Self::Full(bytes) => bytes.is_none(),
            Self::Stream(streaming) => {
                streaming.queued.is_empty() && streaming.idle.is_none() && streaming.next.is_none()
            }
        }
    }
}

impl Streaming {
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, BodyError>>> {
        while let Some(bytes) = self.queued.pop_front() {
            if !bytes.is_empty() {
                return Poll::Ready(Some(Ok(Frame::data(bytes))));
            }
        }
        loop {
            if self.next.is_none() {
                let Some(mut body) = self.idle.take() else {
                    return Poll::Ready(None);
                };
                self.next = Some(Box::pin(async move {
                    let chunk = body.chunk().await;
                    (body, chunk)
                }));
            }
            let Some(next) = self.next.as_mut() else {
                return Poll::Ready(None);
            };
            let (body, chunk) = match next.as_mut().poll(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(done) => done,
            };
            self.next = None;
            match chunk {
                // An empty chunk says nothing: ask again rather than send an empty frame.
                Ok(Some(bytes)) if bytes.is_empty() => self.idle = Some(body),
                Ok(Some(bytes)) => {
                    self.idle = Some(body);
                    return Poll::Ready(Some(Ok(Frame::data(bytes))));
                }
                Ok(None) => return Poll::Ready(None),
                Err(_) => {
                    self.events.report(RelayEvent::Network);
                    return Poll::Ready(Some(Err(BodyError)));
                }
            }
        }
    }
}
