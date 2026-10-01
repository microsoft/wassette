// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

#[cfg(feature = "hyperlight")]
use std::io::Read;
#[cfg(feature = "hyperlight")]
use std::process::{Child, ChildStdin, Command, Stdio};
#[cfg(feature = "hyperlight")]
use std::sync::mpsc;
#[cfg(feature = "hyperlight")]
use std::time::Duration;
use std::time::Instant;

use anyhow::Result;
#[cfg(feature = "hyperlight")]
use anyhow::{Context, ensure};
use tokio_util::sync::CancellationToken;

use crate::{BuildArtifact, BuildLimits, BuildRequest, BuilderConfig};
#[cfg(feature = "hyperlight")]
use crate::{BuildError, BuildErrorKind};

#[cfg(feature = "hyperlight")]
enum Event {
    Input(Result<ChildStdin>),
    Output(Result<Box<BuildArtifact>>),
    Console(Result<usize>),
}

#[cfg(feature = "hyperlight")]
struct KillOnDrop(Child);

#[cfg(feature = "hyperlight")]
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[cfg(feature = "hyperlight")]
impl std::ops::Deref for KillOnDrop {
    type Target = Child;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(feature = "hyperlight")]
impl std::ops::DerefMut for KillOnDrop {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

pub(crate) fn run(
    config: &BuilderConfig,
    limits: &BuildLimits,
    request: BuildRequest,
    cancel: &CancellationToken,
    deadline: Instant,
) -> Result<BuildArtifact> {
    #[cfg(not(feature = "hyperlight"))]
    {
        let _ = (config, limits, request, cancel, deadline);
        anyhow::bail!("builder requires the `hyperlight` feature")
    }
    #[cfg(feature = "hyperlight")]
    {
        let executable = std::env::current_exe().context("locate current builder executable")?;
        run_child(executable, config, limits, request, cancel, deadline)
    }
}

#[cfg(feature = "hyperlight")]
fn run_child(
    executable: std::path::PathBuf,
    config: &BuilderConfig,
    limits: &BuildLimits,
    request: BuildRequest,
    cancel: &CancellationToken,
    deadline: Instant,
) -> Result<BuildArtifact> {
    let stage = tempfile::Builder::new()
        .prefix("wassette-build-")
        .tempdir_in(&config.staging_root)?;
    let remaining = deadline
        .saturating_duration_since(Instant::now())
        .as_millis();
    ensure!(remaining > 0, "build deadline exceeded");
    let mut job_limits = limits.clone();
    job_limits.wall_time_ms = remaining.min(u128::from(limits.wall_time_ms)) as u64;
    let mut command = Command::new(executable);
    command
        .arg(crate::ipc::ARGUMENT)
        .env_clear()
        .current_dir(stage.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = KillOnDrop(command.spawn().context("start isolated builder process")?);
    let stdin = child.stdin.take().context("missing builder stdin")?;
    let stdout = child.stdout.take().context("missing builder stdout")?;
    let stderr = child.stderr.take().context("missing builder stderr")?;
    std::thread::scope(|scope| -> Result<BuildArtifact> {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            supervise_child(
                scope,
                &mut child,
                stdin,
                stdout,
                stderr,
                config,
                job_limits,
                limits,
                request,
                stage.path(),
                cancel,
                deadline,
            )
        }));
        match result {
            Ok(result) => result,
            Err(panic) => {
                let _ = child.kill();
                let _ = child.wait();
                std::panic::resume_unwind(panic)
            }
        }
    })
}

#[cfg(feature = "hyperlight")]
#[allow(clippy::too_many_arguments)]
fn supervise_child<'scope, 'env>(
    scope: &'scope std::thread::Scope<'scope, 'env>,
    child: &mut Child,
    stdin: std::process::ChildStdin,
    stdout: std::process::ChildStdout,
    stderr: std::process::ChildStderr,
    config: &BuilderConfig,
    job_limits: BuildLimits,
    limits: &BuildLimits,
    request: BuildRequest,
    staging: &std::path::Path,
    cancel: &CancellationToken,
    deadline: Instant,
) -> Result<BuildArtifact> {
    let (tx, rx) = mpsc::channel();
    let input_tx = tx.clone();
    let job = crate::ipc::Job {
        config: config.clone(),
        limits: job_limits,
        request,
        staging: staging.to_path_buf(),
    };
    scope.spawn(move || {
        let mut stdin = stdin;
        let result = crate::ipc::write_job(&mut stdin, &job).map(|()| stdin);
        let _ = input_tx.send(Event::Input(result));
    });
    let output_tx = tx.clone();
    let output_limits = limits.clone();
    scope.spawn(move || {
        let result = crate::ipc::read_result(stdout, &output_limits).map(Box::new);
        let _ = output_tx.send(Event::Output(result));
    });
    let console_cap = limits.diagnostics_bytes;
    scope.spawn(move || {
        let _ = tx.send(Event::Console(read_console(stderr, console_cap)));
    });

    let mut input = None;
    let mut output = None;
    let mut console = None;
    let mut failure = None;
    let status = loop {
        if cancel.is_cancelled() {
            failure = Some(anyhow::anyhow!("build cancelled"));
        } else if Instant::now() >= deadline {
            failure = Some(anyhow::anyhow!("build deadline exceeded"));
        }
        while let Ok(event) = rx.try_recv() {
            accept(event, &mut input, &mut output, &mut console, &mut failure);
        }
        if failure.is_some() {
            let kill = child.kill();
            let status = child.wait().context("reap failed builder process")?;
            if !status.success() {
                kill.context("kill builder process")?;
            }
            break status;
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(error) => {
                let _ = child.kill();
                child.wait().context("reap builder after wait error")?;
                return Err(error).context("wait for builder process");
            }
        }
    };
    for event in rx {
        accept(event, &mut input, &mut output, &mut console, &mut failure);
    }
    drop(input);
    if cancel.is_cancelled() {
        return Err(BuildError::new(BuildErrorKind::Cancelled).into());
    }
    if Instant::now() >= deadline || status.code() == Some(124) {
        return Err(BuildError::new(BuildErrorKind::DeadlineExceeded).into());
    }
    if let Some(error) = failure {
        return Err(error);
    }
    let console_bytes = console.context("builder console reader did not finish")??;
    ensure!(
        console_bytes <= limits.diagnostics_bytes,
        BuildError::new(BuildErrorKind::InvalidOutput)
    );
    ensure!(status.success(), "builder process exited unsuccessfully");
    output.context("missing builder result")
}

#[cfg(feature = "hyperlight")]
fn accept(
    event: Event,
    input: &mut Option<ChildStdin>,
    output: &mut Option<BuildArtifact>,
    console: &mut Option<Result<usize>>,
    failure: &mut Option<anyhow::Error>,
) {
    let error = match event {
        Event::Input(Ok(value)) => {
            *input = Some(value);
            None
        }
        Event::Output(Ok(value)) => {
            *output = Some(*value);
            None
        }
        Event::Console(Ok(value)) => {
            *console = Some(Ok(value));
            None
        }
        Event::Input(Err(error)) | Event::Output(Err(error)) => Some(error),
        Event::Console(Err(error)) => Some(error),
    };
    if failure.is_none() {
        *failure = error;
    }
}

#[cfg(feature = "hyperlight")]
fn read_console(mut input: impl Read, cap: usize) -> Result<usize> {
    let mut total = 0usize;
    let mut chunk = [0u8; 4096];
    loop {
        let n = input.read(&mut chunk)?;
        if n == 0 {
            return Ok(total);
        }
        total = total
            .checked_add(n)
            .context("builder console output size overflow")?;
        ensure!(total <= cap, "builder console output exceeded its budget");
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::task::{Context as TaskContext, Poll, Waker};

    use super::*;
    use crate::tests::request;
    use crate::{BuildError, BuildErrorKind, Builder};

    fn setup(wall_time_ms: u64) -> (tempfile::TempDir, Builder) {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("image");
        std::fs::write(&image, b"image").unwrap();
        let config = BuilderConfig {
            initrd_path: image,
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
    fn saturation_returns_busy_without_allocating_staging() {
        let (dir, builder) = setup(5000);
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
    }

    #[test]
    fn cancellation_precedes_busy_and_invalid_input_releases_admission() {
        let (_dir, builder) = setup(5000);
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

    #[cfg(all(feature = "hyperlight", unix))]
    #[test]
    fn parent_unwind_kills_and_reaps_child_process() {
        let child = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let pid = child.id() as libc::pid_t;
        let panic = std::panic::catch_unwind(|| {
            let _child = KillOnDrop(child);
            panic!("simulate supervisor panic");
        });
        assert!(panic.is_err());
        let result = unsafe { libc::kill(pid, 0) };
        assert_eq!(result, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }

    #[cfg(all(feature = "hyperlight", unix))]
    #[tokio::test]
    async fn cancellation_kills_and_reaps_child_process() {
        use std::os::unix::fs::PermissionsExt;

        let (dir, builder) = setup(5000);
        let sleeper = dir.path().join("sleep");
        std::fs::write(&sleeper, "#!/bin/sh\nexec /bin/sleep 30\n").unwrap();
        std::fs::set_permissions(&sleeper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        let cancellation = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            trigger.cancel();
        });
        let result = run_child(
            sleeper,
            &builder.config,
            &builder.limits,
            request(),
            &cancel,
            Instant::now() + Duration::from_secs(5),
        );
        cancellation.join().unwrap();
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

    #[cfg(all(feature = "hyperlight", unix))]
    #[tokio::test]
    async fn timeout_kills_and_reaps_child_process() {
        use std::os::unix::fs::PermissionsExt;

        let (dir, builder) = setup(50);
        let sleeper = dir.path().join("sleep");
        std::fs::write(&sleeper, "#!/bin/sh\nexec /bin/sleep 30\n").unwrap();
        std::fs::set_permissions(&sleeper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let result = run_child(
            sleeper,
            &builder.config,
            &builder.limits,
            request(),
            &CancellationToken::new(),
            Instant::now() + Duration::from_millis(50),
        );
        assert_eq!(
            result
                .unwrap_err()
                .downcast_ref::<BuildError>()
                .unwrap()
                .kind(),
            BuildErrorKind::DeadlineExceeded
        );
        assert_eq!(builder.permits.available_permits(), 1);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }
}
