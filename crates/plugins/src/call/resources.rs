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

use maka_runtime::tools::ToolError;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::task::{TaskTracker, task_tracker::TaskTrackerToken};

#[derive(Default)]
pub struct Resources {
    state: Mutex<State>,
    tasks: TaskTracker,
}
#[derive(Default)]
struct State {
    closed: bool,
    failure: Option<String>,
}

impl Resources {
    pub fn reserve(self: &Arc<Self>) -> Result<Ticket, ToolError> {
        let state = self.state.lock().unwrap();
        if state.closed || self.tasks.len() >= 16 {
            return Err(ToolError::Failed(
                "invocation resources closed or full".into(),
            ));
        }
        Ok(Ticket {
            group: self.clone(),
            _token: self.tasks.token(),
            complete: false,
            started: false,
        })
    }
    pub async fn finish(&self) -> Result<(), ToolError> {
        self.state.lock().unwrap().closed = true;
        self.tasks.close();
        tokio::time::timeout(Duration::from_secs(5), self.tasks.wait())
            .await
            .map_err(|_| ToolError::OutcomeUnknown("plugin resources did not settle".into()))?;
        match &self.state.lock().unwrap().failure {
            Some(error) => Err(ToolError::OutcomeUnknown(error.clone())),
            None => Ok(()),
        }
    }
}
pub struct Ticket {
    group: Arc<Resources>,
    _token: TaskTrackerToken,
    complete: bool,
    started: bool,
}
impl Ticket {
    pub fn start(&mut self) {
        self.started = true;
    }
    pub fn complete(mut self, result: Result<(), String>) {
        if let Err(error) = result {
            self.group
                .state
                .lock()
                .unwrap()
                .failure
                .get_or_insert(error);
        }
        self.complete = true;
    }
}
impl Drop for Ticket {
    fn drop(&mut self) {
        if self.started && !self.complete {
            self.group
                .state
                .lock()
                .unwrap()
                .failure
                .get_or_insert_with(|| "plugin resource worker disappeared".into());
        }
    }
}
