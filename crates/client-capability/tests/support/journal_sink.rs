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

use maka_client_capability::Endpoint;
use maka_event_log::EventLog;
use maka_runtime::event::{CommitError, CommitFuture, EventSink, EventWrite, Fact};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Boundary {
    Success,
    RejectT1,
    UnknownT1,
    CancelAfterT1,
    LostAfterT1,
    LostAfterEffect,
    UnknownT2,
}
pub struct Sink {
    pub log: Arc<EventLog>,
    pub boundary: Boundary,
    pub cancellation: CancellationToken,
    pub endpoint: Endpoint,
}
impl EventSink for Sink {
    fn commit(self: Arc<Self>, write: EventWrite) -> CommitFuture {
        Box::pin(async move {
            let event = write.event();
            let dispatch = matches!(&event.fact, Fact::ToolDispatched { .. });
            if dispatch && self.boundary == Boundary::RejectT1 {
                return Err(CommitError::Rejected("T1 rejected".into()));
            }
            if matches!(&event.fact, Fact::ToolSettled { .. })
                && self.boundary == Boundary::UnknownT2
            {
                return Err(CommitError::OutcomeUnknown("T2 unknown".into()));
            }
            let committed = self.log.clone().commit(write).await?;
            if dispatch {
                match self.boundary {
                    Boundary::UnknownT1 => {
                        return Err(CommitError::OutcomeUnknown(
                            "T1 committed but acknowledgement lost".into(),
                        ));
                    }
                    Boundary::CancelAfterT1 => self.cancellation.cancel(),
                    Boundary::LostAfterT1 => self.endpoint.close(),
                    _ => {}
                }
            }
            Ok(committed)
        })
    }
}
