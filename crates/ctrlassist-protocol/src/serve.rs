//! The server half of the endpoint contract: a query's body, the
//! typed [`Responder`] each decoded rpc carries, and refusals. Reply
//! failures (the querier gone) are logged, never fatal — the caller's
//! own timeout reports them.

use std::fmt;
use std::marker::PhantomData;

use zenoh::bytes::Encoding;
use zenoh::query::Query;

use crate::endpoint::Rpc;
use crate::error::WireError;

/// The reply owed to one query of rpc `R`, handed out with the decoded
/// request. Answering consumes it, so a handler replies once and only
/// with `R`'s response type; one dropped unanswered logs a warning,
/// since its caller is left to time out.
pub struct Responder<R> {
    query: Option<Query>,
    rpc: PhantomData<fn() -> R>,
}

impl<R: Rpc> Responder<R> {
    pub(crate) fn new(query: Query) -> Self {
        Self {
            query: Some(query),
            rpc: PhantomData,
        }
    }

    /// Answer with `R`'s response.
    pub async fn ok(mut self, response: &R::Response) {
        send(&self.take(), encode(response)).await;
    }

    /// Refuse the request.
    pub async fn err(mut self, error: &WireError) {
        reply_err(&self.take(), error).await;
    }

    /// Answer with a handler's outcome: the response, or the refusal.
    pub async fn answer(self, outcome: Result<R::Response, WireError>) {
        match outcome {
            Ok(response) => self.ok(&response).await,
            Err(error) => self.err(&error).await,
        }
    }

    fn take(&mut self) -> Query {
        self.query
            .take()
            .expect("a responder holds its query until answered, and answering consumes it")
    }
}

impl<R> Drop for Responder<R> {
    fn drop(&mut self) {
        if let Some(query) = &self.query {
            tracing::warn!(key = %query.key_expr(), "rpc dropped without a reply");
        }
    }
}

impl<R> fmt::Debug for Responder<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let key = self.query.as_ref().map(|query| query.key_expr().as_str());
        f.debug_tuple("Responder").field(&key).finish()
    }
}

fn encode<T: serde::Serialize>(response: &T) -> Vec<u8> {
    serde_json::to_vec(response).expect(
            "rpc responses are serde structs, string-keyed maps and lists of them, which always encode as JSON",
        )
}

/// A query's body bytes, empty when it carried none.
pub(crate) fn payload(query: &Query) -> Vec<u8> {
    query
        .payload()
        .map(|payload| payload.to_bytes().into_owned())
        .unwrap_or_default()
}

/// Refuse `query` with a typed error: a query no family decoded.
pub(crate) async fn reply_err(query: &Query, error: &WireError) {
    let bytes = serde_json::to_vec(error).expect("WireError always encodes as JSON");
    if let Err(e) = query
        .reply_err(bytes)
        .encoding(Encoding::APPLICATION_JSON)
        .await
    {
        tracing::warn!(key = %query.key_expr(), %e, "rpc refusal failed");
    }
}

async fn send(query: &Query, bytes: Vec<u8>) {
    let key = query.key_expr().clone().into_owned();
    if let Err(e) = query
        .reply(key, bytes)
        .encoding(Encoding::APPLICATION_JSON)
        .await
    {
        tracing::warn!(key = %query.key_expr(), %e, "rpc reply failed");
    }
}
