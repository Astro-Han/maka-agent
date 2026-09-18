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

use futures_util::future::BoxFuture;
use maka_event_log::EventLog;
use maka_plugins::executor::{Error, Outcome, OutputSink};
use maka_runtime::{
    event::{CommitError, EventWrite, Fact, Invocation, InvocationOutcome, RuntimeEvent},
    executor::Output,
};
use std::{collections::HashSet, sync::Arc};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub(super) struct Recorder {
    log: Arc<EventLog>,
    invocation: Invocation,
    cancellation: CancellationToken,
    state: Mutex<State>,
}
#[derive(Default)]
struct State {
    closed: bool,
    bytes: usize,
    events: usize,
    calls: HashSet<String>,
    pending: HashSet<String>,
    failure: Option<CommitError>,
}
impl Recorder {
    pub fn new(
        log: Arc<EventLog>,
        invocation: Invocation,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            log,
            invocation,
            cancellation,
            state: Mutex::default(),
        }
    }
    pub async fn finish(
        &self,
        result: &Result<Outcome, Error>,
        outcome: &InvocationOutcome,
    ) -> Result<(), crate::RunError> {
        // Drain accepted writes before sealing, then reject even retained sinks.
        let mut state = self.state.lock().await;
        state.closed = true;
        if let Some(error) = state.failure.take() {
            return Err(error.into());
        }
        let mut facts = vec![];
        if let (InvocationOutcome::Completed, Ok(Outcome::Completed { text })) = (&outcome, result)
        {
            facts.push(Fact::ExecutorCompleted { text: text.clone() });
        }
        facts.push(Fact::InvocationEnded {
            outcome: outcome.clone(),
        });
        let writes = facts
            .into_iter()
            .map(|fact| EventWrite::plain(RuntimeEvent::new(self.invocation.clone(), fact)))
            .collect::<Result<Vec<_>, _>>()?;
        self.log.append_batch(&writes).await?;
        Ok(())
    }
}
impl OutputSink for Recorder {
    fn emit(&self, output: Output) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            let mut state = self.state.lock().await;
            if state.closed || self.cancellation.is_cancelled() {
                return Err(Error::Cancelled);
            }
            output
                .validate()
                .map_err(|reason| Error::Invalid(reason.into()))?;
            let bytes = serde_json::to_vec(&output)
                .map_err(|error| Error::Invalid(error.to_string()))?
                .len();
            if state.events >= 8192 || bytes > (8 * 1024 * 1024usize).saturating_sub(state.bytes) {
                return Err(Error::Invalid("executor output capacity exceeded".into()));
            }
            match &output {
                Output::ToolStart { tool_call_id, .. }
                    if state.calls.contains(tool_call_id)
                        || state.calls.len() >= 4096
                        || state.pending.len() >= 128 =>
                {
                    return Err(Error::Invalid(
                        "duplicate tool activity ID or capacity exceeded".into(),
                    ));
                }
                Output::ToolProgress { tool_call_id, .. }
                | Output::ToolResult { tool_call_id, .. }
                    if !state.pending.contains(tool_call_id) =>
                {
                    return Err(Error::Invalid(
                        "external tool activity has no pending start".into(),
                    ));
                }
                _ => {}
            }
            let event = EventWrite::plain(RuntimeEvent::new(
                self.invocation.clone(),
                Fact::ExecutorObserved {
                    output: output.clone(),
                },
            ))
            .map_err(|error| Error::Invalid(error.to_string()))?;
            if let Err(error) = self.log.append(&event).await {
                let message = error.to_string();
                state.failure = Some(error);
                self.cancellation.cancel();
                return Err(Error::Persistence(message));
            }
            state.events += 1;
            state.bytes += bytes;
            match output {
                Output::ToolStart { tool_call_id, .. } => {
                    state.calls.insert(tool_call_id.clone());
                    state.pending.insert(tool_call_id);
                }
                Output::ToolResult { tool_call_id, .. } => {
                    state.pending.remove(&tool_call_id);
                }
                _ => {}
            }
            Ok(())
        })
    }
}
