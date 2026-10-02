//! HTTP networking support

pub use http::status::StatusCode;
pub use http::uri::{Authority, PathAndQuery, Uri};

pub use crate::sys::http::client::Client;
pub use crate::sys::http::fields::{HeaderMap, HeaderName, HeaderValue};
pub use crate::sys::http::method::Method;
pub use crate::sys::http::scheme::{InvalidUri, Scheme};
#[doc(inline)]
pub use body::{Body, util::BodyExt};
pub use error::{Error, ErrorCode, Result};
pub use request::Request;
pub use response::Response;

pub mod body {
    //! HTTP body types.
    pub use crate::sys::http::body::*;
}

pub mod error {
    //! The http portion of wstd uses `anyhow::Error` as its `Error` type.
    //!
    //! There are various concrete error types

    pub use crate::http::body::InvalidContentLength;
    pub use crate::sys::http::{ErrorCode, HeaderError};
    pub use anyhow::Context;
    pub use http::header::{InvalidHeaderName, InvalidHeaderValue};
    pub use http::method::InvalidMethod;

    pub type Error = anyhow::Error;
    /// The `http` result type.
    pub type Result<T> = std::result::Result<T, Error>;
}

pub mod request {
    //! HTTP request types.
    pub use crate::sys::http::request::*;
}

pub mod response {
    //! HTTP response types.
    pub use crate::sys::http::response::*;
}

pub mod server {
    //! HTTP servers
    //!
    //! The WASI HTTP server uses the [typed main] idiom, with a `main` function
    //! that takes a [`Request`] and succeeds with a [`Response`], using the
    //! [`http_server`] macro:
    //!
    //! ```no_run
    //! use wstd::http::{Request, Response, Body, Error};
    //! #[wstd::http_server]
    //! async fn main(_request: Request<Body>) -> Result<Response<Body>, Error> {
    //!     Ok(Response::new("Hello!\n".into()))
    //! }
    //! ```
    //!
    //! [typed main]: https://sunfishcode.github.io/typed-main-wasi-presentation/chapter_1.html
    //! [`Request`]: crate::http::Request
    //! [`Responder`]: crate::http::server::Responder
    //! [`Response`]: crate::http::Response
    //! [`http_server`]: crate::http_server

    pub use crate::sys::http::server::*;
}

// Conditionally-compiled declarative macro for HTTP server export
//
// The `#[wstd::http_server]` proc macro delegates to this declarative macro.
// Because `#[macro_export]` macros are compiled in wstd's context, the
// The `wstd_p2` / `wstd_p3` cfgs (set in build.rs) are evaluated against
// wstd's own features and target environment. Consumers don't need to define
// any features themselves.

#[cfg(wstd_p2)]
#[macro_export]
#[doc(hidden)]
macro_rules! __http_server_export {
    (@async { $($run_fn:tt)* }) => {
        const _: () = {
            struct TheServer;

            impl $crate::__internal::wasip2::exports::http::incoming_handler::Guest for TheServer {
                fn handle(
                    wasi_request: $crate::__internal::wasip2::http::types::IncomingRequest,
                    response_out: $crate::__internal::wasip2::http::types::ResponseOutparam
                ) {
                    $($run_fn)*

                    let responder = $crate::http::server::Responder::new(response_out);
                    $crate::runtime::block_on(async move {
                        match $crate::http::request::try_from_incoming(wasi_request) {
                            ::core::result::Result::Ok(request) => match __run(request).await {
                                ::core::result::Result::Ok(response) => { responder.respond(response).await.unwrap() },
                                ::core::result::Result::Err(err) => responder.fail(err),
                            }
                            ::core::result::Result::Err(err) => responder.fail(err),
                        }
                    })
                }
            }

            $crate::__internal::wasip2::http::proxy::export!(TheServer with_types_in $crate::__internal::wasip2);
        };
    };
    (@sync { $($run_fn:tt)* }) => {
        const _: () = {
            struct TheServer;

            impl $crate::__internal::wasip2::exports::http::incoming_handler::Guest for TheServer {
                fn handle(
                    wasi_request: $crate::__internal::wasip2::http::types::IncomingRequest,
                    response_out: $crate::__internal::wasip2::http::types::ResponseOutparam
                ) {
                    $($run_fn)*

                    let responder = $crate::http::server::Responder::new(response_out);
                    $crate::runtime::block_on(async move {
                        match $crate::http::request::try_from_incoming(wasi_request) {
                            ::core::result::Result::Ok(request) => match __run(request) {
                                ::core::result::Result::Ok(response) => { responder.respond(response).await.unwrap() },
                                ::core::result::Result::Err(err) => responder.fail(err),
                            }
                            ::core::result::Result::Err(err) => responder.fail(err),
                        }
                    })
                }
            }

            $crate::__internal::wasip2::http::proxy::export!(TheServer with_types_in $crate::__internal::wasip2);
        };
    };
}

#[cfg(wstd_p3)]
#[macro_export]
#[doc(hidden)]
macro_rules! __http_server_export {
    (@async { $($run_fn:tt)* }) => {
        const _: () = {
            struct TheServer;

            impl $crate::__internal::wasip3::exports::http::handler::Guest for TheServer {
                async fn handle(
                    wasi_request: $crate::__internal::wasip3::http::types::Request,
                ) -> ::core::result::Result<
                    $crate::__internal::wasip3::http::types::Response,
                    $crate::__internal::wasip3::http::types::ErrorCode,
                > {
                    $($run_fn)*

                    let (_writer, completion_reader) = $crate::__internal::wasip3::wit_future::new::<
                        ::core::result::Result<(), $crate::__internal::wasip3::http::types::ErrorCode>,
                    >(|| ::core::result::Result::Ok(()));
                    ::core::mem::drop(_writer);

                    let request = $crate::http::request::try_from_wasi_request(wasi_request, completion_reader)
                        .map_err($crate::http::server::error_to_wasi)?;

                    let response = __run(request).await
                        .map_err($crate::http::server::error_to_wasi)?;

                    $crate::http::server::response_to_wasi(response).await
                }
            }

            $crate::__internal::wasip3::http::service::export!(TheServer with_types_in $crate::__internal::wasip3);
        };
    };
    (@sync { $($run_fn:tt)* }) => {
        const _: () = {
            struct TheServer;

            impl $crate::__internal::wasip3::exports::http::handler::Guest for TheServer {
                async fn handle(
                    wasi_request: $crate::__internal::wasip3::http::types::Request,
                ) -> ::core::result::Result<
                    $crate::__internal::wasip3::http::types::Response,
                    $crate::__internal::wasip3::http::types::ErrorCode,
                > {
                    $($run_fn)*

                    let (_writer, completion_reader) = $crate::__internal::wasip3::wit_future::new::<
                        ::core::result::Result<(), $crate::__internal::wasip3::http::types::ErrorCode>,
                    >(|| ::core::result::Result::Ok(()));
                    ::core::mem::drop(_writer);

                    let request = $crate::http::request::try_from_wasi_request(wasi_request, completion_reader)
                        .map_err($crate::http::server::error_to_wasi)?;

                    let response = __run(request)
                        .map_err($crate::http::server::error_to_wasi)?;

                    $crate::http::server::response_to_wasi(response).await
                }
            }

            $crate::__internal::wasip3::http::service::export!(TheServer with_types_in $crate::__internal::wasip3);
        };
    };
}
