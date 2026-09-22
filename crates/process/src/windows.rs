/*
 * Licensed to the Apache Software Foundation (ASF) under one
 * or more contributor license agreements.  See the NOTICE file
 * distributed with this work for additional information
 * regarding copyright ownership.  The ASF licenses this file
 * to you under the Apache License, Version 2.0 (the
 * "License"); you may not use this file except in compliance
 * with the License.  You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing,
 * software distributed under the License is distributed on an
 * "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
 * KIND, either express or implied.  See the License for the
 * specific language governing permissions and limitations
 * under the License.
 */

use crate::{
    PipeEvent, failed,
    output::{Captured, Outcome, Termination},
    tail,
};
use maka_runtime::shell_run::PipeStream;
use maka_runtime::tools::ToolError;
use std::{
    io,
    os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::ExitStatusExt,
    },
    process::ExitStatus,
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use windows_sys::Win32::{
    Foundation::{HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::Threading::{GetExitCodeProcess, WaitForSingleObject},
};

pub(crate) mod attributes;
pub(crate) mod job;
pub(crate) mod pipe;
pub(crate) mod spawn;

pub(crate) async fn run(
    plan: crate::Command,
    timeout_ms: Option<u64>,
    cancellation: CancellationToken,
    observer: Option<tokio::sync::mpsc::Sender<PipeEvent>>,
) -> Result<Captured, ToolError> {
    if cancellation.is_cancelled() {
        return Err(failed("Shell cancelled before spawn"));
    }
    let mut plan = plan
        .prepare()
        .await
        .map_err(|error| failed(error.to_string()))?;
    if let Some(launch) = plan.take_launch() {
        let spawned = launch
            .runner
            .pipes(launch.endpoint, plan, launch.capabilities)
            .await
            .map_err(|error| failed(format!("Shell spawn failed: {error}")))?;
        return capture_managed(spawned, timeout_ms, cancellation, observer).await;
    }
    let spawn::Spawned {
        child,
        stdout,
        stderr,
    } = spawn::spawn(&plan, &cancellation)
        .await
        .map_err(|e| failed(format!("Shell spawn failed: {e}")))?;
    if let Some(observer) = &observer {
        let _ = observer.try_send(PipeEvent::Started);
    }
    let exited = CancellationToken::new();
    let lifecycle = async {
        let result = settle(&child, timeout_ms, cancellation).await;
        exited.cancel();
        result
    };
    let (outcome, stdout, stderr) = tokio::join!(
        lifecycle,
        tail::capture(
            stdout,
            exited.clone(),
            observer.clone().map(|o| (o, PipeStream::Stdout))
        ),
        tail::capture(
            stderr,
            exited.clone(),
            observer.map(|o| (o, PipeStream::Stderr))
        )
    );
    Ok(Captured::new(outcome?, stdout, stderr))
}

async fn capture_managed(
    spawned: crate::pipe::Spawned,
    timeout: Option<u64>,
    cancellation: CancellationToken,
    observer: Option<tokio::sync::mpsc::Sender<PipeEvent>>,
) -> Result<Captured, ToolError> {
    let crate::pipe::Spawned {
        mut child,
        stdin,
        stdout,
        stderr,
    } = spawned;
    drop(stdin);
    if let Some(observer) = &observer {
        let _ = observer.try_send(PipeEvent::Started);
    }
    let exited = CancellationToken::new();
    let lifecycle = async {
        let result = async {
            let reason = tokio::select! {
                biased;
                status = child.wait() => return status.map(Outcome::Exited).map_err(unknown),
                _ = cancellation.cancelled() => Termination::Cancelled,
                _ = crate::timeout(timeout) => Termination::Timeout,
            };
            child.terminate().map_err(unknown)?;
            child.wait().await.map_err(unknown)?;
            Ok(Outcome::Interrupted {
                reason,
                signal_applied: true,
            })
        }
        .await;
        exited.cancel();
        result
    };
    let (outcome, stdout, stderr) = tokio::join!(
        lifecycle,
        tail::capture(
            stdout,
            exited.clone(),
            observer.clone().map(|o| (o, PipeStream::Stdout))
        ),
        tail::capture(
            stderr,
            exited.clone(),
            observer.map(|o| (o, PipeStream::Stderr))
        )
    );
    Ok(Captured::new(outcome?, stdout, stderr))
}

pub(crate) async fn wait_process(process: &OwnedHandle) -> io::Result<ExitStatus> {
    loop {
        if let Some(status) = try_wait_process(process)? {
            return Ok(status);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

pub(crate) fn try_wait_process(process: &OwnedHandle) -> io::Result<Option<ExitStatus>> {
    // SAFETY: the owned handle pins the exact process throughout this wait.
    match unsafe { WaitForSingleObject(process.as_raw_handle(), 0) } {
        WAIT_OBJECT_0 => {
            let mut code = 0;
            // SAFETY: signalled process and valid output pointer.
            unsafe {
                checked(GetExitCodeProcess(process.as_raw_handle(), &mut code))?;
            }
            Ok(Some(ExitStatus::from_raw(code)))
        }
        WAIT_TIMEOUT => Ok(None),
        _ => Err(io::Error::last_os_error()),
    }
}

async fn settle(
    child: &spawn::Child,
    timeout: Option<u64>,
    cancellation: CancellationToken,
) -> Result<Outcome, ToolError> {
    let reason = tokio::select! {
        biased;
        status = wait_process(&child.process) => {
            let status = status.map_err(unknown)?;
            child.job.preserve_descendants().map_err(unknown)?;
            return Ok(Outcome::Exited(status));
        },
        _ = cancellation.cancelled() => Termination::Cancelled,
        _ = crate::timeout(timeout) => Termination::Timeout,
    };
    // Cancellation owns the entire Job. Windows has no POSIX TERM grace;
    // job termination is explicit, while normal shell exit preserves descendants.
    child
        .job
        .terminate(match reason {
            Termination::Timeout => 124,
            Termination::Cancelled => 130,
        })
        .map_err(unknown)?;
    tokio::time::timeout(Duration::from_secs(2), async {
        wait_process(&child.process).await?;
        while !child.job.is_empty()? {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok::<_, io::Error>(())
    })
    .await
    .map_err(|_| ToolError::CleanupUnconfirmed("Shell Job exit not confirmed".into()))?
    .map_err(unknown)?;
    Ok(Outcome::Interrupted {
        reason,
        signal_applied: true,
    })
}

fn unknown(error: io::Error) -> ToolError {
    ToolError::CleanupUnconfirmed(format!("Shell process tree exit not confirmed: {error}"))
}

pub(crate) fn checked(success: i32) -> io::Result<()> {
    if success == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub(crate) fn owned(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: callers transfer a successful Windows API result exactly once.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}
