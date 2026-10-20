use http::StatusCode;
use wasip2::http::types::IncomingResponse;

use super::fields::header_map_from_wasi;
use crate::http::HeaderMap;
use crate::http::body::{Body, BodyHint};
use crate::http::error::Error;

pub use http::response::{Builder, Response};

pub(crate) fn try_from_incoming(incoming: IncomingResponse) -> Result<Response<Body>, Error> {
    let headers: HeaderMap = header_map_from_wasi(incoming.headers())?;
    // TODO: Does WASI guarantee that the incoming status is valid?
    let status = StatusCode::from_u16(incoming.status())
        .map_err(|err| anyhow::anyhow!("wasi provided invalid status code ({err})"))?;

    let hint = BodyHint::from_headers(&headers)?;
    // `body_stream` is a child of `incoming_body` which means we cannot
    // drop the parent before we drop the child
    let incoming_body = incoming
        .consume()
        .expect("cannot call `consume` twice on incoming response");
    let body = Body::from_incoming(incoming_body, hint);

    let mut builder = Response::builder().status(status);
    // The [`http::response::Builder`] keeps internal state of whether the
    // builder has errored, which is only reachable by passing
    // [`Builder::header`] an erroring `TryInto<HeaderName>` or
    // `TryInto<HeaderValue>`. Since the `Builder::header` method is never
    // used, we know `Builder::headers_mut` will never give the None case, nor
    // will `Builder::body` give the error case. So, rather than treat those
    // as control flow, we unwrap if this invariant is ever broken because
    // that would only be possible due to some unrecoverable bug in wstd,
    // rather than incorrect use or invalid input.
    *builder.headers_mut().expect("builder has not errored") = headers;
    Ok(builder
        .body(body)
        .expect("response builder should not error"))
}
