// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::io::Read;
use std::process::{ChildStdin, Command, Stdio};
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use tokio_util::sync::CancellationToken;

use crate::{
    BuildArtifact, BuildError, BuildErrorKind, BuildLimits, BuildRequest, BuilderConfig, artifact,
    evidence, file_digest, ipc,
};

enum Event {
    Input(Result<ChildStdin>),
    Output(Result<(Vec<u8>, String)>),
    Console(Result<Vec<u8>>),
}

pub(crate) fn run(
    config: &BuilderConfig,
    limits: &BuildLimits,
    request: BuildRequest,
    cancel: &CancellationToken,
    deadline: Instant,
) -> Result<BuildArtifact> {
    ensure!(
        file_digest(&config.helper_path, 256 * 1024 * 1024, cancel, deadline)?
            == config.helper_sha256,
        "builder helper digest mismatch"
    );
    let stage = tempfile::Builder::new()
        .prefix("wassette-build-")
        .tempdir_in(&config.staging_root)?;
    let mut job = ipc::Job {
        config: config.clone(),
        limits: limits.clone(),
        request,
        staging: stage.path().to_path_buf(),
    };
    let remaining = deadline
        .saturating_duration_since(Instant::now())
        .as_millis();
    ensure!(!cancel.is_cancelled(), "build cancelled");
    ensure!(remaining > 0, "build deadline exceeded");
    job.limits.wall_time_ms = remaining.min(u128::from(limits.wall_time_ms)) as u64;
    let mut command = Command::new(&config.helper_path);
    command
        .arg(ipc::ARGUMENT)
        .env_clear()
        .current_dir(stage.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .context("start packaged builder helper (check executable/platform/signing)")?;
    let stdin = child.stdin.take().context("missing helper stdin")?;
    let stdout = child.stdout.take().context("missing helper stdout")?;
    let stderr = child.stderr.take().context("missing helper stderr")?;
    let diagnostics_used = AtomicUsize::new(0);
    // All pipe threads finish only after the process is killed/reaped. The
    // staging directory and caller's semaphore permit outlive this scope.
    let output = std::thread::scope(|scope| -> Result<(Vec<u8>, String)> {
        let (tx, rx) = mpsc::channel();
        let input_tx = tx.clone();
        let job = &job;
        scope.spawn(move || {
            let mut stdin = stdin;
            let result = ipc::write_job(&mut stdin, job).map(|()| stdin);
            let _ = input_tx.send(Event::Input(result));
        });
        let output_tx = tx.clone();
        let diagnostics_used = &diagnostics_used;
        scope.spawn(move || {
            let _ = output_tx.send(Event::Output(ipc::read_result(
                stdout,
                limits,
                diagnostics_used,
            )));
        });
        scope.spawn(move || {
            let _ = tx.send(Event::Console(read_console(
                stderr,
                limits.diagnostics_bytes,
                diagnostics_used,
            )));
        });
        let mut input = None;
        let mut output = None;
        let mut console = None;
        let mut failure = None;
        let mut exit = None;
        while exit.is_none() {
            if cancel.is_cancelled() {
                failure = Some(anyhow::anyhow!("build cancelled"));
            } else if Instant::now() >= deadline {
                failure = Some(anyhow::anyhow!("build deadline exceeded"));
            }
            while let Ok(event) = rx.try_recv() {
                accept(event, &mut input, &mut output, &mut console, &mut failure);
            }
            if failure.is_some() {
                // wait is mandatory even when kill races with ordinary exit.
                let kill = child.kill();
                let status = child.wait().context("reap failed builder helper")?;
                if !status.success() {
                    kill.context("kill builder helper")?;
                }
                exit = Some(status);
            } else {
                match child.try_wait() {
                    Ok(Some(status)) => exit = Some(status),
                    Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                    Err(error) => {
                        let _ = child.kill();
                        child.wait().context("reap helper after wait error")?;
                        return Err(error).context("wait for builder helper");
                    }
                }
            }
        }
        // Receiving also waits for pipe readers: no success before exit/EOF.
        for event in rx {
            accept(event, &mut input, &mut output, &mut console, &mut failure);
        }
        drop(input);
        if exit
            .as_ref()
            .is_some_and(|status| status.code() == Some(124))
        {
            return Err(BuildError::new(BuildErrorKind::DeadlineExceeded).into());
        }
        if let Some(error) = failure {
            return Err(error);
        }
        let _console = console.context("helper console reader did not finish")?;
        let status = exit.context("helper was not reaped")?;
        ensure!(status.success(), "builder helper exited unsuccessfully");
        let (wasm, diagnostics) = output.context("missing helper result")?;
        Ok((wasm, diagnostics))
    })?;
    let wasm = artifact::name_component(output.0, &job.request, limits.wasm_bytes, || {
        ensure!(
            Instant::now() < deadline,
            "build deadline exceeded during metadata transform"
        );
        ensure!(!cancel.is_cancelled(), "build cancelled");
        Ok(())
    })
    .map_err(|error| {
        BuildError::explain(
            &error,
            BuildErrorKind::InvalidOutput,
            limits.diagnostics_bytes,
        )
    })?;
    ensure!(
        Instant::now() < deadline,
        "build deadline exceeded during metadata transform"
    );
    ensure!(!cancel.is_cancelled(), "build cancelled");
    Ok(BuildArtifact {
        wasm,
        diagnostics: output.1,
        evidence: evidence(config, &job.request),
    })
}

fn accept(
    event: Event,
    input: &mut Option<ChildStdin>,
    output: &mut Option<(Vec<u8>, String)>,
    console: &mut Option<Vec<u8>>,
    failure: &mut Option<anyhow::Error>,
) {
    let error = match event {
        Event::Input(Ok(value)) => {
            *input = Some(value);
            None
        }
        Event::Output(Ok(value)) => {
            *output = Some(value);
            None
        }
        Event::Console(Ok(value)) => {
            *console = Some(value);
            None
        }
        Event::Input(Err(error)) | Event::Output(Err(error)) | Event::Console(Err(error)) => {
            Some(error)
        }
    };
    if failure.is_none() {
        *failure = error;
    }
}

fn read_console(
    mut input: impl Read,
    cap: usize,
    diagnostics_used: &AtomicUsize,
) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = input.read(&mut chunk)?;
        if n == 0 {
            return Ok(bytes);
        }
        ipc::reserve_diagnostics(diagnostics_used, n, cap)?;
        bytes.extend_from_slice(&chunk[..n]);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::future::Future;
    use std::os::unix::fs::PermissionsExt;
    use std::task::{Context as TaskContext, Poll, Waker};

    use super::*;
    use crate::tests::request;
    use crate::{Builder, sha256};

    fn setup(script: &str, wall_time_ms: u64) -> (tempfile::TempDir, Builder) {
        let dir = tempfile::tempdir().unwrap();
        let helper = dir.path().join("helper");
        std::fs::write(&helper, script).unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let image = dir.path().join("image");
        std::fs::write(&image, b"image").unwrap();
        let config = BuilderConfig {
            helper_path: helper,
            helper_sha256: sha256(script.as_bytes()),
            initrd_path: image,
            initrd_sha256: sha256(b"image"),
            staging_root: dir.path().into(),
            wit_dependencies: vec![],
            rust_crates: vec![],
        };
        let builder = Builder::new(
            config,
            BuildLimits {
                wall_time_ms,
                ..BuildLimits::default()
            },
        )
        .unwrap();
        (dir, builder)
    }

    #[test]
    fn saturation_returns_busy_in_one_poll_without_registering_a_waiter() {
        let (dir, builder) = setup("#!/bin/sh\nexit 99\n", 5000);
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
        assert_eq!(serde_json::to_value(error.kind()).unwrap(), "busy");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);

        // Keep the completed future alive: it must not claim a released permit.
        drop(active);
        assert_eq!(builder.permits.available_permits(), 1);
        let next = builder.permits.clone().try_acquire_owned().unwrap();
        drop(next);
    }

    #[test]
    fn cancellation_precedes_busy_and_invalid_input_releases_admission() {
        let (_dir, builder) = setup("#!/bin/sh\nexit 99\n", 5000);
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

    #[tokio::test]
    async fn timeout_kills_reaps_and_removes_staging() {
        let (dir, builder) = setup("#!/bin/sh\nexec /bin/sleep 30\n", 100);
        let start = Instant::now();
        let error = builder
            .build(request(), CancellationToken::new())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("deadline"));
        assert!(start.elapsed() < Duration::from_secs(3));
        assert_eq!(builder.permits.available_permits(), 1);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[tokio::test]
    async fn dropped_future_still_reaps_and_releases_permit() {
        let (dir, builder) = setup("#!/bin/sh\nexec /bin/sleep 30\n", 5000);
        let clone = builder.clone();
        let task =
            tokio::spawn(async move { clone.build(request(), CancellationToken::new()).await });
        while builder.permits.available_permits() != 0 {
            tokio::task::yield_now().await;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
        task.abort();
        let _ = task.await;
        tokio::time::timeout(Duration::from_secs(3), async {
            while builder.permits.available_permits() == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[tokio::test]
    async fn fails_closed_on_bad_helper_digest_and_protocol() {
        let (_dir, mut builder) = setup("#!/bin/sh\nprintf wrong000\n", 2000);
        assert!(
            builder
                .build(request(), CancellationToken::new())
                .await
                .is_err()
        );
        Arc::make_mut(&mut builder.config).helper_sha256 = "0".repeat(64);
        let error = builder
            .build(request(), CancellationToken::new())
            .await
            .unwrap_err();
        let typed = error.downcast_ref::<BuildError>().unwrap();
        assert_eq!(typed.kind(), BuildErrorKind::Unavailable);
        assert!(typed.diagnostic().is_none());
        assert!(!format!("{error:#}").contains("digest"));
    }

    #[tokio::test]
    async fn excessive_console_is_killed_without_filling_disk() {
        let (dir, builder) = setup(
            "#!/bin/sh\nprintf WSBLD003\nwhile :; do printf 012345678901234567890123456789 >&2; done\n",
            2000,
        );
        let error = builder
            .build(request(), CancellationToken::new())
            .await
            .unwrap_err();
        let typed = error.downcast_ref::<BuildError>().unwrap();
        assert_eq!(typed.kind(), BuildErrorKind::InvalidOutput);
        assert!(typed.diagnostic().unwrap().contains("budget"));
        assert_eq!(builder.permits.available_permits(), 1);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[tokio::test]
    async fn cancellation_reaps_before_returning() {
        let (dir, builder) = setup("#!/bin/sh\nexec /bin/sleep 30\n", 5000);
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        let cancellation = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            trigger.cancel();
        };
        let (result, ()) = tokio::join!(builder.build(request(), cancel), cancellation);
        assert_eq!(
            result
                .unwrap_err()
                .downcast_ref::<BuildError>()
                .unwrap()
                .kind(),
            BuildErrorKind::Cancelled
        );
        assert_eq!(builder.permits.available_permits(), 1);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    use std::sync::Arc;
}
