//! HTTP servers (p3)
//!
//! In p3, the handler trait is `async fn handle(Request) -> Result<Response, ErrorCode>`.
//! The `#[wstd::http_server]` macro generates the appropriate export code.

use super::fields::header_map_to_wasi;
use crate::http::{Body, Error, Response, error::ErrorCode};
use http::header::CONTENT_LENGTH;
use wasip3::http::types::{Response as WasiResponse, Trailers};

/// Convert a wstd Response into a p3 WASI Response for the handler.
#[doc(hidden)]
pub async fn response_to_wasi<B: Into<Body>>(
    response: Response<B>,
) -> Result<WasiResponse, ErrorCode> {
    let headers = response.headers();
    let status = response.status().as_u16();

    let wasi_headers = header_map_to_wasi(headers)
        .map_err(|_| ErrorCode::InternalError(Some("header error".to_string())))?;

    let mut body: Body = response.into_body().into();

    if let Some(len) = body.content_length() {
        let mut buffer = itoa::Buffer::new();
        wasi_headers
            .append(CONTENT_LENGTH.as_str(), buffer.format(len).as_bytes())
            .map_err(|_| {
                ErrorCode::InternalError(Some("content-length header error".to_string()))
            })?;
    }

    // Create body stream and write body data.
    // The write must be spawned as a separate task because the stream reader
    // can only make progress once the response is returned to the runtime.
    // Writing inline would deadlock: write waits for reader, reader waits
    // for response, response waits for write.
    let body_bytes = body
        .contents()
        .await
        .map_err(|e| ErrorCode::InternalError(Some(format!("collecting body: {e:?}"))))?
        .to_vec();

    let body_reader = if body_bytes.is_empty() {
        None
    } else {
        let (writer, reader) = wasip3::wit_stream::new::<u8>();
        wit_bindgen::rt::async_support::spawn_local(async move {
            let mut writer = writer;
            let remaining = writer.write_all(body_bytes).await;
            if !remaining.is_empty() {
                #[cfg(debug_assertions)]
                panic!(
                    "response body write incomplete: {} bytes remaining",
                    remaining.len()
                );
            }
        });
        Some(reader)
    };

    let (trailers_writer, trailers_reader) =
        wasip3::wit_future::new::<Result<Option<Trailers>, ErrorCode>>(|| Ok(None));
    drop(trailers_writer);

    let (wasi_response, _completion) =
        WasiResponse::new(wasi_headers, body_reader, trailers_reader);
    wasi_response
        .set_status_code(status)
        .map_err(|()| ErrorCode::InternalError(Some("status code error".to_string())))?;

    Ok(wasi_response)
}

/// Convert an error to a p3 ErrorCode.
#[doc(hidden)]
pub fn error_to_wasi(err: Error) -> ErrorCode {
    match err.downcast_ref::<ErrorCode>() {
        Some(e) => e.clone(),
        None => ErrorCode::InternalError(Some(format!("{err:?}"))),
    }
}
