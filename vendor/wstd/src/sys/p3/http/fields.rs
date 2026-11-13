pub use http::header::{HeaderMap, HeaderName, HeaderValue};

use crate::http::Error;
use crate::http::error::Context;
use wasip3::http::types::Fields;

pub(crate) fn header_map_from_wasi(wasi_fields: Fields) -> Result<HeaderMap, Error> {
    let mut output = HeaderMap::new();
    for (key, value) in wasi_fields.copy_all() {
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
        wasi_fields
            .append(key.as_str(), value.as_bytes())
            .with_context(|| format!("wasi rejected header `{key}: {value:?}`"))?
    }
    Ok(wasi_fields)
}
