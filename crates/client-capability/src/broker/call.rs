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

use super::{CallError, FormHandler, Progress, state::Inner};
use maka_runtime::capability::{AdmissionEvidence, CallResult};
use std::sync::Arc;
use tokio::sync::{oneshot, watch};
use tokio_util::sync::CancellationToken;

struct Guard {
    inner: Arc<Inner>,
    id: String,
}
impl Drop for Guard {
    fn drop(&mut self) {
        self.inner.stop(&self.id, CallError::Cancelled, true);
    }
}

pub struct PendingCall {
    guard: Guard,
    accepted: oneshot::Receiver<Result<AdmissionEvidence, CallError>>,
    result: oneshot::Receiver<Result<CallResult, CallError>>,
    progress: watch::Receiver<Option<Progress>>,
    provider: CancellationToken,
}
pub struct AcceptedCall {
    guard: Guard,
    evidence: AdmissionEvidence,
    result: oneshot::Receiver<Result<CallResult, CallError>>,
    progress: watch::Receiver<Option<Progress>>,
    provider: CancellationToken,
}
impl PendingCall {
    pub(super) fn new(
        inner: Arc<Inner>,
        id: String,
        accepted: oneshot::Receiver<Result<AdmissionEvidence, CallError>>,
        result: oneshot::Receiver<Result<CallResult, CallError>>,
        progress: watch::Receiver<Option<Progress>>,
        provider: CancellationToken,
    ) -> Self {
        Self {
            guard: Guard { inner, id },
            accepted,
            result,
            progress,
            provider,
        }
    }
    pub fn invocation_id(&self) -> &str {
        &self.guard.id
    }
    pub fn provider_signal(&self) -> CancellationToken {
        self.provider.clone()
    }
    pub async fn accepted(self) -> Result<AcceptedCall, CallError> {
        let evidence = self
            .accepted
            .await
            .map_err(|_| CallError::CapabilityLost)??;
        Ok(AcceptedCall {
            guard: self.guard,
            evidence,
            result: self.result,
            progress: self.progress,
            provider: self.provider,
        })
    }
}
impl AcceptedCall {
    pub fn invocation_id(&self) -> &str {
        &self.guard.id
    }
    pub fn evidence(&self) -> &AdmissionEvidence {
        &self.evidence
    }
    pub fn provider_signal(&self) -> CancellationToken {
        self.provider.clone()
    }
    pub fn progress(&self) -> watch::Receiver<Option<Progress>> {
        self.progress.clone()
    }

    /// Caller completes policy and durable dispatch admission before this call.
    /// No execution deadline runs while the caller is deciding.
    pub async fn admit(self) -> Result<CallResult, CallError> {
        self.start().await
    }

    /// Cross the admission cut synchronously, then wait outside Host policy
    /// locks. Dropping the returned future still cancels the owned call.
    pub fn start(self) -> impl Future<Output = Result<CallResult, CallError>> + Send {
        self.guard.inner.admit(&self.guard.id, None);
        async move {
            let result = self.result.await;
            drop(self.guard);
            result.map_err(|_| CallError::OutcomeUnknown("result owner was lost"))?
        }
    }

    /// The handler owns canonical interaction publication and withdrawal.
    pub async fn admit_with_interactions(
        self,
        handler: Arc<dyn FormHandler>,
    ) -> Result<CallResult, CallError> {
        self.guard.inner.admit(&self.guard.id, Some(handler));
        self.result
            .await
            .map_err(|_| CallError::OutcomeUnknown("result owner was lost"))?
    }
}
