use crate::http::{Error, error::Context as _};
use crate::io::AsyncInputStream;

pub use ::http_body::{Body as HttpBody, Frame, SizeHint};
pub use bytes::Bytes;
use http::header::CONTENT_LENGTH;
use http_body_util::{BodyExt, combinators::UnsyncBoxBody};
use std::fmt;

type HeaderMap = http::header::HeaderMap;

pub mod util {
    pub use http_body_util::*;
}

#[derive(Debug)]
pub struct Body(BodyInner);

#[derive(Debug)]
enum BodyInner {
    Boxed(UnsyncBoxBody<Bytes, Error>),
    P3Stream(P3StreamBody),
    Complete {
        data: Bytes,
        trailers: Option<HeaderMap>,
    },
}

struct P3StreamBody {
    reader: Option<wasip3::wit_bindgen::rt::async_support::StreamReader<u8>>,
    size_hint: BodyHint,
}

impl fmt::Debug for P3StreamBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("P3StreamBody").finish()
    }
}

impl Body {
    pub(crate) fn from_p3_stream(
        reader: wasip3::wit_bindgen::rt::async_support::StreamReader<u8>,
        size_hint: BodyHint,
    ) -> Self {
        Body(BodyInner::P3Stream(P3StreamBody {
            reader: Some(reader),
            size_hint,
        }))
    }

    /// Convert this `Body` into an `UnsyncBoxBody<Bytes, Error>`.
    pub fn into_boxed_body(self) -> UnsyncBoxBody<Bytes, Error> {
        fn map_e(_: std::convert::Infallible) -> Error {
            unreachable!()
        }
        match self.0 {
            BodyInner::P3Stream(p3) => {
                let stream = AsyncInputStream::new(p3.reader.unwrap());
                use futures_lite::stream::StreamExt;
                http_body_util::StreamBody::new(stream.into_stream().map(|res| {
                    res.map(|bytevec| Frame::data(Bytes::from_owner(bytevec)))
                        .map_err(Into::into)
                }))
                .boxed_unsync()
            }
            BodyInner::Complete { data, trailers } => http_body_util::Full::new(data)
                .map_err(map_e)
                .with_trailers(async move { Ok(trailers).transpose() })
                .boxed_unsync(),
            BodyInner::Boxed(b) => b,
        }
    }

    pub async fn contents(&mut self) -> Result<&[u8], Error> {
        match &mut self.0 {
            BodyInner::Complete { data, .. } => Ok(&*data),
            inner => {
                let mut prev = BodyInner::Complete {
                    data: Bytes::new(),
                    trailers: None,
                };
                std::mem::swap(inner, &mut prev);

                // For p3 streams, read directly using the async read method
                if let BodyInner::P3Stream(p3) = prev {
                    let mut stream = AsyncInputStream::new(p3.reader.unwrap());
                    let mut all_data = Vec::new();
                    let mut buf = vec![0u8; 64 * 1024];
                    loop {
                        match stream.read(&mut buf).await {
                            Ok(0) => break,
                            Ok(n) => all_data.extend_from_slice(&buf[..n]),
                            Err(e) => return Err(Error::from(e).context("reading p3 body stream")),
                        }
                    }
                    *inner = BodyInner::Complete {
                        data: Bytes::from(all_data),
                        trailers: None,
                    };
                    return Ok(match inner {
                        BodyInner::Complete { data, .. } => &*data,
                        _ => unreachable!(),
                    });
                }

                let boxed_body = match prev {
                    BodyInner::Boxed(b) => b,
                    BodyInner::Complete { .. } => unreachable!(),
                    BodyInner::P3Stream(_) => unreachable!(),
                };
                let collected = boxed_body.collect().await?;
                let trailers = collected.trailers().cloned();
                *inner = BodyInner::Complete {
                    data: collected.to_bytes(),
                    trailers,
                };
                Ok(match inner {
                    BodyInner::Complete { data, .. } => &*data,
                    _ => unreachable!(),
                })
            }
        }
    }

    pub fn content_length(&self) -> Option<u64> {
        match &self.0 {
            BodyInner::Boxed(b) => b.size_hint().exact(),
            BodyInner::Complete { data, .. } => Some(data.len() as u64),
            BodyInner::P3Stream(p3) => p3.size_hint.content_length(),
        }
    }

    pub fn empty() -> Self {
        Body(BodyInner::Complete {
            data: Bytes::new(),
            trailers: None,
        })
    }

    pub async fn str_contents(&mut self) -> Result<&str, Error> {
        let bs = self.contents().await?;
        std::str::from_utf8(bs).context("decoding body contents as string")
    }

    #[cfg(feature = "json")]
    pub fn from_json<T: serde::Serialize>(data: &T) -> Result<Self, serde_json::Error> {
        Ok(Self::from(serde_json::to_vec(data)?))
    }

    #[cfg(feature = "json")]
    pub async fn json<T: for<'a> serde::Deserialize<'a>>(&mut self) -> Result<T, Error> {
        let str = self.str_contents().await?;
        serde_json::from_str(str).context("decoding body contents as json")
    }

    pub fn from_stream<S>(stream: S) -> Self
    where
        S: futures_lite::Stream + Send + 'static,
        <S as futures_lite::Stream>::Item: Into<Bytes>,
    {
        use futures_lite::StreamExt;
        Self::from_http_body(http_body_util::StreamBody::new(
            stream.map(|bs| Ok::<_, Error>(Frame::data(bs.into()))),
        ))
    }

    pub fn from_try_stream<S, D, E>(stream: S) -> Self
    where
        S: futures_lite::Stream<Item = Result<D, E>> + Send + 'static,
        D: Into<Bytes>,
        E: std::error::Error + Send + Sync + 'static,
    {
        use futures_lite::StreamExt;
        Self::from_http_body(http_body_util::StreamBody::new(
            stream.map(|bs| Ok::<_, Error>(Frame::data(bs?.into()))),
        ))
    }

    pub fn from_http_body<B>(http_body: B) -> Self
    where
        B: HttpBody + Send + 'static,
        <B as HttpBody>::Data: Into<Bytes>,
        <B as HttpBody>::Error: Into<Error>,
    {
        use util::BodyExt;
        Body(BodyInner::Boxed(
            http_body
                .map_frame(|f| f.map_data(Into::into))
                .map_err(Into::into)
                .boxed_unsync(),
        ))
    }
}

impl From<()> for Body {
    fn from(_: ()) -> Body {
        Body::empty()
    }
}
impl From<&[u8]> for Body {
    fn from(bytes: &[u8]) -> Body {
        Body::from(bytes.to_owned())
    }
}
impl From<Vec<u8>> for Body {
    fn from(bytes: Vec<u8>) -> Body {
        Body::from(Bytes::from(bytes))
    }
}
impl From<Bytes> for Body {
    fn from(data: Bytes) -> Body {
        Body(BodyInner::Complete {
            data,
            trailers: None,
        })
    }
}
impl From<&str> for Body {
    fn from(data: &str) -> Body {
        Body::from(data.as_bytes())
    }
}
impl From<String> for Body {
    fn from(data: String) -> Body {
        Body::from(data.into_bytes())
    }
}

impl From<crate::io::AsyncInputStream> for Body {
    fn from(r: crate::io::AsyncInputStream) -> Body {
        use futures_lite::stream::StreamExt;
        Body(BodyInner::Boxed(http_body_util::BodyExt::boxed_unsync(
            http_body_util::StreamBody::new(r.into_stream().map(|res| {
                res.map(|bytevec| Frame::data(Bytes::from_owner(bytevec)))
                    .map_err(Into::into)
            })),
        )))
    }
}

#[derive(Clone, Copy, Debug)]
pub enum BodyHint {
    ContentLength(u64),
    Unknown,
}

impl BodyHint {
    pub fn from_headers(headers: &HeaderMap) -> Result<Self, InvalidContentLength> {
        if let Some(val) = headers.get(CONTENT_LENGTH) {
            let len = std::str::from_utf8(val.as_ref())
                .map_err(|_| InvalidContentLength)?
                .parse::<u64>()
                .map_err(|_| InvalidContentLength)?;
            Ok(BodyHint::ContentLength(len))
        } else {
            Ok(BodyHint::Unknown)
        }
    }
    fn content_length(&self) -> Option<u64> {
        match self {
            BodyHint::ContentLength(l) => Some(*l),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct InvalidContentLength;
impl fmt::Display for InvalidContentLength {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Invalid Content-Length header")
    }
}
impl std::error::Error for InvalidContentLength {}
