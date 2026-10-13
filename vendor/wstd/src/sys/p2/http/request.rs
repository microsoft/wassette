use super::fields::{header_map_from_wasi, header_map_to_wasi};
use super::method::{from_wasi_method, to_wasi_method};
use super::scheme::{from_wasi_scheme, to_wasi_scheme};
use crate::http::{
    Authority, HeaderMap, PathAndQuery, Uri,
    body::{Body, BodyHint},
    error::{Context, Error, ErrorCode},
};
use wasip2::http::outgoing_handler::OutgoingRequest;
use wasip2::http::types::IncomingRequest;

pub use http::request::{Builder, Request};

// TODO: go back and add json stuff???

pub(crate) fn try_into_outgoing<T>(request: Request<T>) -> Result<(OutgoingRequest, T), Error> {
    let wasi_req = OutgoingRequest::new(header_map_to_wasi(request.headers())?);

    let (parts, body) = request.into_parts();

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
        .unwrap_or(wasip2::http::types::Scheme::Https);
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

    // All done; request is ready for send-off
    Ok((wasi_req, body))
}

/// This is used by the `http_server` macro.
#[doc(hidden)]
pub fn try_from_incoming(incoming: IncomingRequest) -> Result<Request<Body>, Error> {
    let headers: HeaderMap = header_map_from_wasi(incoming.headers())
        .context("headers provided by wasi rejected by http::HeaderMap")?;

    let method =
        from_wasi_method(incoming.method()).map_err(|_| ErrorCode::HttpRequestMethodInvalid)?;
    let scheme = incoming
        .scheme()
        .map(|scheme| {
            from_wasi_scheme(scheme).context("scheme provided by wasi rejected by http::Scheme")
        })
        .transpose()?;
    let authority = incoming
        .authority()
        .map(|authority| {
            Authority::from_maybe_shared(authority)
                .context("authority provided by wasi rejected by http::Authority")
        })
        .transpose()?;
    let path_and_query = incoming
        .path_with_query()
        .map(|path_and_query| {
            PathAndQuery::from_maybe_shared(path_and_query)
                .context("path and query provided by wasi rejected by http::PathAndQuery")
        })
        .transpose()?;

    let hint = BodyHint::from_headers(&headers)?;

    // `body_stream` is a child of `incoming_body` which means we cannot
    // drop the parent before we drop the child
    let incoming_body = incoming
        .consume()
        .expect("`consume` should not have been called previously on this incoming-request");
    let body = Body::from_incoming(incoming_body, hint);

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
