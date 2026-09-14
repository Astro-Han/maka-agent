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

use maka_runtime::workhub::ActionId;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub enum CancellationCause {
    Runtime,
    WorkhubStop { action_id: ActionId },
    WorkhubCorrection { action_id: ActionId },
}

#[derive(Clone)]
pub struct RunCancellation {
    token: CancellationToken,
    cause: Arc<Mutex<Option<CancellationCause>>>,
}

impl RunCancellation {
    pub(crate) fn new(token: CancellationToken) -> Self {
        Self {
            token,
            cause: Arc::new(Mutex::new(None)),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }

    pub fn cancel(&self) {
        self.cancel_with(CancellationCause::Runtime);
    }

    /// The first observed cancellation owns attribution. A later control
    /// request cannot relabel a manual stop or an already-cancelled parent.
    pub fn cancel_with(&self, cause: CancellationCause) {
        let mut first = self.cause.lock().unwrap();
        if first.is_none() {
            *first = Some(if self.token.is_cancelled() {
                CancellationCause::Runtime
            } else {
                cause
            });
        }
        self.token.cancel();
    }

    pub(crate) fn token(&self) -> &CancellationToken {
        &self.token
    }

    pub(crate) fn source(&self) -> String {
        match self.cause.lock().unwrap().as_ref() {
            Some(CancellationCause::WorkhubCorrection { action_id }) => {
                maka_runtime::workhub::correction_abort_source(action_id)
            }
            Some(CancellationCause::WorkhubStop { action_id }) => {
                maka_runtime::workhub::stop_abort_source(action_id)
            }
            Some(CancellationCause::Runtime) | None => "runtime_cancellation".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_cancellation_keeps_its_source_including_parent_cancellation() {
        for prior in [false, true] {
            let parent = CancellationToken::new();
            let owner = RunCancellation::new(parent.child_token());
            if prior {
                parent.cancel();
            } else {
                owner.cancel();
            }
            owner.cancel_with(CancellationCause::WorkhubStop {
                action_id: "later".parse().unwrap(),
            });
            assert_eq!(owner.source(), "runtime_cancellation");
        }
        let owner = RunCancellation::new(CancellationToken::new());
        owner.cancel_with(CancellationCause::WorkhubStop {
            action_id: "first".parse().unwrap(),
        });
        owner.cancel_with(CancellationCause::WorkhubStop {
            action_id: "second".parse().unwrap(),
        });
        owner.cancel();
        assert!(owner.is_cancelled());
        assert_eq!(
            owner.source(),
            maka_runtime::workhub::stop_abort_source(&"first".parse().unwrap())
        );
    }
}
