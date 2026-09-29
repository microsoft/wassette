use crate::http::request::try_into_outgoing;
use crate::http::response::try_from_incoming;
use crate::http::{Body, Error, Request, Response};
use crate::io::AsyncPollable;
use crate::time::Duration;
use wasip2::http::types::RequestOptions as WasiRequestOptions;

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
        let (wasi_req, body) = try_into_outgoing(req)?;
        let body = body.into();
        let wasi_body = wasi_req.body().unwrap();

        // 1. Start sending the request head
        let res = wasip2::http::outgoing_handler::handle(wasi_req, self.wasi_options()?)?;

        let ((), body) = futures_lite::future::try_zip(
            async move {
                // 3. send the body:
                body.send(wasi_body).await
            },
            async move {
                // 4. Receive the response
                AsyncPollable::new(res.subscribe()).wait_for().await;

                // NOTE: the first `unwrap` is to ensure readiness, the second `unwrap`
                // is to trap if we try and get the response more than once. The final
                // `?` is to raise the actual error if there is one.
                let res = res.get().unwrap().unwrap()?;
                try_from_incoming(res)
            },
        )
        .await?;
        Ok(body)
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

    fn wasi_options(&self) -> Result<Option<WasiRequestOptions>, crate::http::Error> {
        self.options
            .as_ref()
            .map(RequestOptions::to_wasi)
            .transpose()
    }
}

#[derive(Default, Debug, Clone)]
struct RequestOptions {
    connect_timeout: Option<Duration>,
    first_byte_timeout: Option<Duration>,
    between_bytes_timeout: Option<Duration>,
}

impl RequestOptions {
    fn to_wasi(&self) -> Result<WasiRequestOptions, crate::http::Error> {
        let wasi = WasiRequestOptions::new();
        if let Some(timeout) = self.connect_timeout {
            wasi.set_connect_timeout(Some(timeout.0)).map_err(|()| {
                anyhow::Error::msg(
                    "wasi-http implementation does not support connect timeout option",
                )
            })?;
        }
        if let Some(timeout) = self.first_byte_timeout {
            wasi.set_first_byte_timeout(Some(timeout.0)).map_err(|()| {
                anyhow::Error::msg(
                    "wasi-http implementation does not support first byte timeout option",
                )
            })?;
        }
        if let Some(timeout) = self.between_bytes_timeout {
            wasi.set_between_bytes_timeout(Some(timeout.0))
                .map_err(|()| {
                    anyhow::Error::msg(
                        "wasi-http implementation does not support between byte timeout option",
                    )
                })?;
        }
        Ok(wasi)
    }
}
