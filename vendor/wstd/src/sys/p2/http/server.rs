//! HTTP servers (wasip2 backend).

use super::fields::header_map_to_wasi;
use crate::http::{Body, Error, Response, error::ErrorCode};
use wasip2::exports::http::incoming_handler::ResponseOutparam;
use wasip2::http::types::OutgoingResponse;

/// For use by the [`http_server`] macro only.
///
/// [`http_server`]: crate::http_server
#[doc(hidden)]
#[must_use]
pub struct Responder {
    outparam: ResponseOutparam,
}

impl Responder {
    /// This is used by the `http_server` macro.
    #[doc(hidden)]
    pub async fn respond<B: Into<Body>>(self, response: Response<B>) -> Result<(), Error> {
        let headers = response.headers();
        let status = response.status().as_u16();

        let wasi_headers = header_map_to_wasi(headers).expect("header error");

        // Consume the `response` and prepare to write the body.
        let body = response.into_body().into();

        let wasi_response = OutgoingResponse::new(wasi_headers);

        // Unwrap because `StatusCode` has already validated the status.
        wasi_response.set_status_code(status).unwrap();

        // Unwrap because we can be sure we only call these once.
        let wasi_body = wasi_response.body().unwrap();

        // Set the outparam to the response, which allows wasi-http to send
        // the response status and headers.
        ResponseOutparam::set(self.outparam, Ok(wasi_response));

        // Then send the body. The response will be fully sent once this
        // future is ready.
        body.send(wasi_body).await
    }

    /// This is used by the `http_server` macro.
    #[doc(hidden)]
    pub fn new(outparam: ResponseOutparam) -> Self {
        Self { outparam }
    }

    /// This is used by the `http_server` macro.
    #[doc(hidden)]
    pub fn fail(self, err: Error) {
        let e = match err.downcast_ref::<ErrorCode>() {
            Some(e) => e.clone(),
            None => ErrorCode::InternalError(Some(format!("{err:?}"))),
        };
        ResponseOutparam::set(self.outparam, Err(e));
    }
}
