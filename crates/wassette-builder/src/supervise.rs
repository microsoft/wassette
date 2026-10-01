// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::time::Instant;

use anyhow::{Result, bail};
use tokio_util::sync::CancellationToken;

use crate::{BuildArtifact, BuildLimits, BuildRequest, BuilderConfig};

pub(crate) fn run(
    config: &BuilderConfig,
    limits: &BuildLimits,
    request: BuildRequest,
    cancel: &CancellationToken,
    deadline: Instant,
) -> Result<BuildArtifact> {
    #[cfg(feature = "hyperlight")]
    {
        let stage = tempfile::Builder::new()
            .prefix("wassette-build-")
            .tempdir_in(&config.staging_root)?;
        let mut limits = limits.clone();
        let remaining = deadline
            .saturating_duration_since(Instant::now())
            .as_millis();
        if remaining == 0 {
            bail!("build deadline exceeded");
        }
        limits.wall_time_ms = remaining.min(u128::from(limits.wall_time_ms)) as u64;

        crate::helper::build(config, &limits, request, stage.path(), cancel, deadline)
    }
    #[cfg(not(feature = "hyperlight"))]
    {
        let _ = (config, limits, request, cancel, deadline);
        bail!("builder requires the `hyperlight` feature")
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::task::{Context as TaskContext, Poll, Waker};

    use super::*;
    use crate::tests::request;
    use crate::{BuildError, BuildErrorKind, Builder};

    fn setup() -> (tempfile::TempDir, Builder) {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("image");
        std::fs::write(&image, b"image").unwrap();
        let config = BuilderConfig {
            initrd_path: image,
            staging_root: dir.path().into(),
            wit_dependencies: vec![],
            rust_crates: vec![],
        };
        let builder = Builder::new(config, BuildLimits::default()).unwrap();
        (dir, builder)
    }

    #[test]
    fn saturation_returns_busy_without_allocating_staging() {
        let (dir, builder) = setup();
        let active = builder.permits.clone().try_acquire_owned().unwrap();
        let mut input = request();
        input.source = "x".repeat(builder.limits.source_bytes);
        let mut future = std::pin::pin!(builder.build(input, CancellationToken::new()));
        let mut context = TaskContext::from_waker(Waker::noop());
        let Poll::Ready(Err(error)) = future.as_mut().poll(&mut context) else {
            panic!("saturated builder must not leave a pending request");
        };
        let error = BuildError::from_error(&error).unwrap();
        assert_eq!(error.kind(), BuildErrorKind::Busy);
        assert!(error.diagnostic().is_none());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);

        drop(active);
        assert_eq!(builder.permits.available_permits(), 1);
        let next = builder.permits.clone().try_acquire_owned().unwrap();
        drop(next);
    }

    #[test]
    fn cancellation_precedes_busy_and_invalid_input_releases_admission() {
        let (_dir, builder) = setup();
        let active = builder.permits.clone().try_acquire_owned().unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let mut future = std::pin::pin!(builder.build(request(), cancel));
        let mut context = TaskContext::from_waker(Waker::noop());
        let Poll::Ready(Err(error)) = future.as_mut().poll(&mut context) else {
            panic!("pre-cancelled build must finish in one poll");
        };
        assert_eq!(
            BuildError::from_error(&error).unwrap().kind(),
            BuildErrorKind::Cancelled
        );
        drop(active);

        let mut input = request();
        input.source.clear();
        let mut future = std::pin::pin!(builder.build(input, CancellationToken::new()));
        let Poll::Ready(Err(error)) = future.as_mut().poll(&mut context) else {
            panic!("invalid input must release its admission synchronously");
        };
        assert_eq!(
            BuildError::from_error(&error).unwrap().kind(),
            BuildErrorKind::InvalidRequest
        );
        assert_eq!(builder.permits.available_permits(), 1);
    }
}
