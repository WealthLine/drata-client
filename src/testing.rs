//! Test support: a scripted transport, a client on it, and a tiny executor.

use std::{
    collections::VecDeque,
    error::Error as StdError,
    future::{Future, ready},
    pin::pin,
    sync::Mutex,
    task::{Context, Poll, Waker},
    time::Duration,
};

use crate::{
    ApiKey, Client, Error, Region, TransportError,
    transport::{Method, Request, Response, Transport},
};

pub(crate) const KEY: &str = "sk-live-do-not-print-0123456789";
pub(crate) const RECORD_EMAIL: &str = "alice@example.com";

/// What the scripted transport saw of one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Seen {
    pub(crate) method: Method,
    pub(crate) url: String,
    pub(crate) authorization: String,
    pub(crate) body: Option<Vec<u8>>,
}

/// A transport that answers from a script and records what it was sent.
#[derive(Default)]
pub(crate) struct Scripted {
    answers: Mutex<VecDeque<Result<Response, TransportError>>>,
    /// Requests from this many on never get an answer.
    hang_from: Option<usize>,
    seen: Mutex<Vec<Seen>>,
    slept: Mutex<Vec<Duration>>,
}

/// Counts only: `seen` holds each request's `Authorization` header, and
/// printing it would make the client's `Debug` look like it leaks the key.
impl std::fmt::Debug for Scripted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scripted")
            .field(
                "requests_seen",
                &self.seen.lock().map_or(0, |seen| seen.len()),
            )
            .finish_non_exhaustive()
    }
}

impl Scripted {
    pub(crate) fn new(answers: impl IntoIterator<Item = Result<Response, TransportError>>) -> Self {
        Self {
            answers: Mutex::new(answers.into_iter().collect()),
            ..Self::default()
        }
    }

    /// A script whose request number `request`, counting from zero, and every
    /// later one never get an answer, as when a run is killed mid-call.
    pub(crate) fn hanging_from(
        request: usize,
        answers: impl IntoIterator<Item = Result<Response, TransportError>>,
    ) -> Self {
        Self {
            hang_from: Some(request),
            ..Self::new(answers)
        }
    }

    pub(crate) fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    pub(crate) fn slept(&self) -> Vec<Duration> {
        self.slept.lock().unwrap().clone()
    }
}

impl Transport for &Scripted {
    fn send(&self, request: Request<'_>) -> impl Future<Output = Result<Response, TransportError>> {
        let mut seen = self.seen.lock().unwrap();
        let number = seen.len();
        seen.push(Seen {
            method: request.method,
            url: request.url,
            authorization: format!("Bearer {}", request.key.expose()),
            body: request.body,
        });
        drop(seen);
        if self.hang_from.is_some_and(|from| number >= from) {
            return Reply(None);
        }
        Reply(Some(
            self.answers
                .lock()
                .unwrap()
                .pop_front()
                .expect("the test should script an answer for every request"),
        ))
    }

    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> {
        self.slept.lock().unwrap().push(duration);
        ready(())
    }
}

/// A scripted answer, ready at once, or never when it holds nothing.
struct Reply(Option<Result<Response, TransportError>>);

impl Future for Reply {
    type Output = Result<Response, TransportError>;

    fn poll(mut self: std::pin::Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        self.0.take().map_or(Poll::Pending, Poll::Ready)
    }
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "a scripted answer sits beside `no_answer()` in one list of results"
)]
pub(crate) fn answer(status: u16, body: &str) -> Result<Response, TransportError> {
    Ok(Response {
        status,
        retry_after: None,
        body: body.as_bytes().to_vec(),
    })
}

pub(crate) fn no_answer() -> Result<Response, TransportError> {
    Err(TransportError::new("connection reset"))
}

pub(crate) fn client(transport: &Scripted) -> Client<&Scripted> {
    Client::new(transport, ApiKey::new(KEY), Region::Us).with_base_url("https://drata.test/v2")
}

/// Polls a future until it finishes or stops making progress, then drops
/// it. `None` means it was dropped unfinished, as when a run is killed.
pub(crate) fn run_until_stalled<F: Future>(future: F) -> Option<F::Output> {
    let mut future = pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    (0..1_000).find_map(|_| match future.as_mut().poll(&mut context) {
        Poll::Ready(output) => Some(output),
        Poll::Pending => None,
    })
}

/// Runs a future that never waits on anything outside itself, which is true
/// of every future here because the scripted transport answers at once.
pub(crate) fn run<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
    }
}

/// Every way an error can be printed: `Display`, `Debug` and its sources.
pub(crate) fn every_rendering(error: &Error) -> String {
    let mut out = vec![format!("{error}"), format!("{error:?}")];
    let mut source = error.source();
    while let Some(cause) = source {
        out.extend([format!("{cause}"), format!("{cause:?}")]);
        source = cause.source();
    }
    out.join("\n")
}

/// The body of the request at `index`, as text.
pub(crate) fn sent_body(transport: &Scripted, index: usize) -> String {
    let seen = transport.seen();
    let body = seen
        .get(index)
        .and_then(|seen| seen.body.clone())
        .expect("the test should look only at requests that carried a body");
    String::from_utf8(body).expect("every body the client sends should be JSON text")
}
