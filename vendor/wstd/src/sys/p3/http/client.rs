use super::request::try_into_wasi_request;
use super::response::try_from_wasi_response;
use crate::http::{Body, Error, Request, Response};
use crate::time::Duration;

/// An HTTP client.
#[derive(Debug, Clone)]
pub struct Client {
    options: Option<RequestOptions>,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    /// Create a new instance of `Client`
    pub fn new() -> Self {
        Self { options: None }
    }

    /// Send an HTTP request.
    pub async fn send<B: Into<Body>>(&self, req: Request<B>) -> Result<Response<Body>, Error> {
        let parts = try_into_wasi_request(req, self.options.as_ref())?;

        // Drive the request send and the body write concurrently. The
        // `wasip3::http::client::send` future is what consumes the body
        // stream's reader; if we wrote the body to completion *first*,
        // `write_all` would block waiting for a consumer that hasn't been
        // started yet (deadlock for any non-empty body).
        let body_fut = async move {
            if let Some(mut body_writer) = parts.body_writer {
                let mut body = parts.body;
                let body_bytes = body.contents().await?;
                if !body_bytes.is_empty() {
                    let remaining = body_writer.write_all(body_bytes.to_vec()).await;
                    if !remaining.is_empty() {
                        return Err(anyhow::anyhow!("failed to write full request body"));
                    }
                }
                drop(body_writer);
            }
            Ok::<(), Error>(())
        };

        let send_fut = wasip3::http::client::send(parts.request);
        let (send_res, body_res) = futures_lite::future::zip(send_fut, body_fut).await;
        body_res?;
        let wasi_resp = send_res?;

        // Create a completion future for consuming the response body
        let (_completion_writer, completion_reader) =
            wasip3::wit_future::new::<Result<(), super::ErrorCode>>(|| Ok(()));
        drop(_completion_writer);

        try_from_wasi_response(wasi_resp, completion_reader)
    }

    /// Set timeout on connecting to HTTP server
    pub fn set_connect_timeout(&mut self, d: impl Into<Duration>) {
        self.options_mut().connect_timeout = Some(d.into());
    }

    /// Set timeout on recieving first byte of the Response body
    pub fn set_first_byte_timeout(&mut self, d: impl Into<Duration>) {
        self.options_mut().first_byte_timeout = Some(d.into());
    }

    /// Set timeout on recieving subsequent chunks of bytes in the Response body stream
    pub fn set_between_bytes_timeout(&mut self, d: impl Into<Duration>) {
        self.options_mut().between_bytes_timeout = Some(d.into());
    }

    fn options_mut(&mut self) -> &mut RequestOptions {
        match &mut self.options {
            Some(o) => o,
            uninit => {
                *uninit = Some(RequestOptions::default());
                uninit.as_mut().unwrap()
            }
        }
    }
}

#[derive(Default, Debug, Clone)]
pub(crate) struct RequestOptions {
    pub(crate) connect_timeout: Option<Duration>,
    pub(crate) first_byte_timeout: Option<Duration>,
    pub(crate) between_bytes_timeout: Option<Duration>,
}
