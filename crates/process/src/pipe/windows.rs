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

use super::Spawned;
use crate::{
    Command,
    windows::{job::Job, pipe, wait_process},
};
use std::{io, os::windows::io::OwnedHandle, process::ExitStatus, time::Duration};

pub struct Child {
    process: OwnedHandle,
    job: Job,
    pid: u32,
    status: Option<ExitStatus>,
    cleaned: bool,
    runner: Option<crate::bootstrap::Runner>,
}
pub async fn spawn(plan: Command) -> io::Result<Spawned> {
    let mut plan = plan.prepare().await?;
    if let Some(launch) = plan.take_launch() {
        return launch
            .runner
            .pipes(launch.endpoint, plan, launch.capabilities)
            .await;
    }
    let (stdin, stdin_peer) = pipe::input().await?;
    let (stdout, stdout_peer) = pipe::output().await?;
    let (stderr, stderr_peer) = pipe::output().await?;
    let job = Job::new()?;
    let (process, pid) = crate::windows::spawn::launch(
        &plan,
        &job,
        [&stdin_peer, &stdout_peer, &stderr_peer],
        crate::windows::spawn::Console::None,
    )?;
    Ok(Spawned {
        stdin,
        stdout,
        stderr,
        child: Child {
            process,
            job,
            pid,
            status: None,
            cleaned: false,
            runner: None,
        },
    })
}
impl Child {
    pub(crate) fn from_runner(
        process: OwnedHandle,
        pid: u32,
        job: Job,
        runner: crate::bootstrap::Runner,
    ) -> Self {
        Self {
            process,
            pid,
            job,
            status: None,
            cleaned: false,
            runner: Some(runner),
        }
    }

    pub fn id(&self) -> u32 {
        self.pid
    }
    pub fn terminate(&mut self) -> io::Result<()> {
        self.job.terminate(130)
    }
    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        if self.status.is_none() {
            self.status = Some(wait_process(&self.process).await?);
        }
        if !self.cleaned {
            self.terminate()?;
            tokio::time::timeout(Duration::from_secs(2), async {
                while !self.job.is_empty()? {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Ok::<_, io::Error>(())
            })
            .await
            .map_err(|_| io::Error::other("process Job exit is unconfirmed"))??;
            self.cleaned = true;
        }
        if let Some(runner) = &mut self.runner {
            runner.finish().await?;
        }
        Ok(self.status.expect("root exited"))
    }
}
// Job's kill-on-close is emergency cleanup. Only wait() confirms tree exit.
