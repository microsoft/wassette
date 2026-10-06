use super::fields::{header_map_from_wasi, header_map_to_wasi};
use super::method::{from_wasi_method, to_wasi_method};
use super::scheme::{from_wasi_scheme, to_wasi_scheme};
use crate::http::{
    Authority, HeaderMap, PathAndQuery, Uri,
    body::{Body, BodyHint},
    error::{Context, Error, ErrorCode},
};

use wasip3::http::types::{
    Request as WasiRequest, RequestOptions as WasiRequestOptions, Scheme as WasiScheme,
};

pub use http::request::{Builder, Request};

/// Result of converting an http::Request into a p3 WASI Request.
pub(crate) struct WasiRequestParts {
    pub request: WasiRequest,
    pub body: Body,
    pub body_writer: Option<wasip3::wit_bindgen::rt::async_support::StreamWriter<u8>>,
    pub _completion: wasip3::wit_bindgen::rt::async_support::FutureReader<Result<(), ErrorCode>>,
}

/// Convert an http::Request into a p3 WASI Request for sending.
pub(crate) fn try_into_wasi_request<T: Into<Body>>(
    request: Request<T>,
    request_options: Option<&super::client::RequestOptions>,
) -> Result<WasiRequestParts, Error> {
    let headers = header_map_to_wasi(request.headers())?;
    let (parts, body) = request.into_parts();
    let body: Body = body.into();

    // Create trailers future (no trailers for now)
    let (trailers_writer, trailers_reader) = wasip3::wit_future::new::<
        Result<Option<wasip3::http::types::Trailers>, ErrorCode>,
    >(|| Ok(None));
    drop(trailers_writer);

    // Create body stream — keep the writer for the caller to send body data
    let (body_writer, body_reader) = if body.content_length() == Some(0) {
        (None, None)
    } else {
        let (writer, reader) = wasip3::wit_stream::new::<u8>();
        (Some(writer), Some(reader))
    };

    let options = WasiRequestOptions::new();
    if let Some(opts) = request_options {
        if let Some(timeout) = opts.connect_timeout {
            let _ = options.set_connect_timeout(Some(timeout.0));
        }
        if let Some(timeout) = opts.first_byte_timeout {
            let _ = options.set_first_byte_timeout(Some(timeout.0));
        }
        if let Some(timeout) = opts.between_bytes_timeout {
            let _ = options.set_between_bytes_timeout(Some(timeout.0));
        }
    }

    let (wasi_req, completion) =
        WasiRequest::new(headers, body_reader, trailers_reader, Some(options));

    // Set the HTTP method
    let method = to_wasi_method(parts.method);
    wasi_req
        .set_method(&method)
        .map_err(|()| anyhow::anyhow!("method rejected by wasi-http: {method:?}"))?;

    // Set the url scheme
    let scheme = parts
        .uri
        .scheme()
        .map(to_wasi_scheme)
        .unwrap_or(WasiScheme::Https);
    wasi_req
        .set_scheme(Some(&scheme))
        .map_err(|()| anyhow::anyhow!("scheme rejected by wasi-http: {scheme:?}"))?;

    // Set authority
    let authority = parts.uri.authority().map(Authority::as_str);
    wasi_req
        .set_authority(authority)
        .map_err(|()| anyhow::anyhow!("authority rejected by wasi-http {authority:?}"))?;

    // Set the url path + query string
    if let Some(p_and_q) = parts.uri.path_and_query() {
        wasi_req
            .set_path_with_query(Some(p_and_q.as_str()))
            .map_err(|()| anyhow::anyhow!("path and query rejected by wasi-http {p_and_q:?}"))?;
    }

    Ok(WasiRequestParts {
        request: wasi_req,
        body,
        body_writer,
        _completion: completion,
    })
}

/// Convert a p3 WASI Request into an http::Request (for the server handler).
#[doc(hidden)]
pub fn try_from_wasi_request(
    incoming: WasiRequest,
    completion: wasip3::wit_bindgen::rt::async_support::FutureReader<Result<(), ErrorCode>>,
) -> Result<Request<Body>, Error> {
    let headers: HeaderMap = header_map_from_wasi(incoming.get_headers())
        .context("headers provided by wasi rejected by http::HeaderMap")?;

    let method =
        from_wasi_method(incoming.get_method()).map_err(|_| ErrorCode::HttpRequestMethodInvalid)?;
    let scheme = incoming
        .get_scheme()
        .map(|scheme| {
            from_wasi_scheme(scheme).context("scheme provided by wasi rejected by http::Scheme")
        })
        .transpose()?;
    let authority = incoming
        .get_authority()
        .map(|authority| {
            Authority::from_maybe_shared(authority)
                .context("authority provided by wasi rejected by http::Authority")
        })
        .transpose()?;
    let path_and_query = incoming
        .get_path_with_query()
        .map(|path_and_query| {
            PathAndQuery::from_maybe_shared(path_and_query)
                .context("path and query provided by wasi rejected by http::PathAndQuery")
        })
        .transpose()?;

    let hint = BodyHint::from_headers(&headers)?;

    // Consume the request body
    let (body_stream, _trailers_future) = WasiRequest::consume_body(incoming, completion);
    let body = Body::from_p3_stream(body_stream, hint);

    let mut uri = Uri::builder();
    if let Some(scheme) = scheme {
        uri = uri.scheme(scheme);
    }
    if let Some(authority) = authority {
        uri = uri.authority(authority);
    }
    if let Some(path_and_query) = path_and_query {
        uri = uri.path_and_query(path_and_query);
    }
    let uri = uri.build().context("building uri from wasi")?;

    let mut request = Request::builder().method(method).uri(uri);
    if let Some(headers_mut) = request.headers_mut() {
        *headers_mut = headers;
    }
    request.body(body).context("building request from wasi")
}
