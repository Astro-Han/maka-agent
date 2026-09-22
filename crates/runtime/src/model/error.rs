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

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// A provider failure with protocol-level evidence for safe replay.
#[derive(Clone, Debug, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[error("provider failed ({reason:?}): {message}")]
pub struct ProviderFailure {
    reason: ProviderFailureReason,
    message: String,
    replay_safe: bool,
    retry_after_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderFailureReason {
    Network,
    RateLimit,
    ProviderUnavailable,
    StreamTruncated,
}

impl ProviderFailure {
    pub fn new(
        reason: ProviderFailureReason,
        message: impl Into<String>,
        replay_safe: bool,
        retry_after_ms: Option<u64>,
    ) -> Self {
        let mut message = message.into();
        if message.len() > 4096 {
            let mut end = 4096;
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            message.truncate(end);
        }
        Self {
            reason,
            message,
            replay_safe,
            retry_after_ms: retry_after_ms.filter(|value| (1..=2_147_483_647).contains(value)),
        }
    }
    pub fn reason(&self) -> ProviderFailureReason {
        self.reason
    }

    pub fn replay_safe(&self) -> bool {
        self.replay_safe
    }

    pub fn retry_after(&self) -> Option<Duration> {
        self.retry_after_ms.map(Duration::from_millis)
    }

    pub fn validate(&self) -> Result<(), ModelError> {
        if self.message.len() > 4096
            || self
                .retry_after_ms
                .is_some_and(|delay| !(1..=2_147_483_647).contains(&delay))
        {
            return Err(ModelError::Adapter(
                "invalid provider failure evidence".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, thiserror::Error)]
pub enum ModelError {
    #[error("model request cancelled")]
    Cancelled,
    #[error("model stream idle timeout exceeded")]
    TimedOut,
    #[error("model input exceeds provider capacity (observed output: {observed_output})")]
    ContextOverflow { observed_output: bool },
    #[error(transparent)]
    Provider(ProviderFailure),
    #[error("model adapter failed: {0}")]
    Adapter(String),
}
