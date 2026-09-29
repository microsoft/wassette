use wasip2::http::types::Method as WasiMethod;

pub use http::Method;
use http::method::InvalidMethod;

pub(crate) fn to_wasi_method(value: Method) -> WasiMethod {
    match value {
        Method::GET => WasiMethod::Get,
        Method::HEAD => WasiMethod::Head,
        Method::POST => WasiMethod::Post,
        Method::PUT => WasiMethod::Put,
        Method::DELETE => WasiMethod::Delete,
        Method::CONNECT => WasiMethod::Connect,
        Method::OPTIONS => WasiMethod::Options,
        Method::TRACE => WasiMethod::Trace,
        Method::PATCH => WasiMethod::Patch,
        other => WasiMethod::Other(other.as_str().to_owned()),
    }
}

pub(crate) fn from_wasi_method(value: WasiMethod) -> Result<Method, InvalidMethod> {
    Ok(match value {
        WasiMethod::Get => Method::GET,
        WasiMethod::Head => Method::HEAD,
        WasiMethod::Post => Method::POST,
        WasiMethod::Put => Method::PUT,
        WasiMethod::Delete => Method::DELETE,
        WasiMethod::Connect => Method::CONNECT,
        WasiMethod::Options => Method::OPTIONS,
        WasiMethod::Trace => Method::TRACE,
        WasiMethod::Patch => Method::PATCH,
        WasiMethod::Other(s) => Method::from_bytes(s.as_bytes())?,
    })
}
