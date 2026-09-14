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

use maka_runtime::event::Invocation;
use std::{collections::HashSet, sync::Arc};
use tokio::task::JoinHandle;

use crate::{RunCancellation, RunError};

/// An admitted invocation's lifetime owner. Cancellation is a request, not a
/// terminal result; wait observes completion only after durable finalization.
pub struct RunningInvocation {
    invocation: Invocation,
    tool_names: Arc<HashSet<String>>,
    cancellation: RunCancellation,
    worker: JoinHandle<Result<Invocation, RunError>>,
}

impl RunningInvocation {
    pub(crate) fn new(
        invocation: Invocation,
        tool_names: Arc<HashSet<String>>,
        cancellation: RunCancellation,
        worker: JoinHandle<Result<Invocation, RunError>>,
    ) -> Self {
        Self {
            invocation,
            tool_names,
            cancellation,
            worker,
        }
    }

    pub fn invocation(&self) -> &Invocation {
        &self.invocation
    }

    /// The admitted Run's complete catalog, including deferred tools. Session
    /// configuration changes and discovery visibility do not replace it.
    pub fn tool_names(&self) -> &Arc<HashSet<String>> {
        &self.tool_names
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub fn cancellation(&self) -> RunCancellation {
        self.cancellation.clone()
    }

    pub async fn wait(mut self) -> Result<Invocation, RunError> {
        (&mut self.worker)
            .await
            .map_err(|error| RunError::Internal(error.to_string()))?
    }
}

impl Drop for RunningInvocation {
    fn drop(&mut self) {
        // JoinHandle drop detaches, never aborts the worker that owns admission.
        self.cancellation.cancel();
    }
}
