pub use http::header::{HeaderMap, HeaderName, HeaderValue};

use crate::http::{Error, error::Context};
use wasip2::http::types::Fields;

pub(crate) fn header_map_from_wasi(wasi_fields: Fields) -> Result<HeaderMap, Error> {
    let mut output = HeaderMap::new();
    for (key, value) in wasi_fields.entries() {
        let key =
            HeaderName::from_bytes(key.as_bytes()).with_context(|| format!("header name {key}"))?;
        let value =
            HeaderValue::from_bytes(&value).with_context(|| format!("header value for {key}"))?;
        output.append(key, value);
    }
    Ok(output)
}

pub(crate) fn header_map_to_wasi(header_map: &HeaderMap) -> Result<Fields, Error> {
    let wasi_fields = Fields::new();
    for (key, value) in header_map {
        // Unwrap because `HeaderMap` has already validated the headers.
        wasi_fields
            .append(key.as_str(), value.as_bytes())
            .with_context(|| format!("wasi rejected header `{key}: {value:?}`"))?
    }
    Ok(wasi_fields)
}
