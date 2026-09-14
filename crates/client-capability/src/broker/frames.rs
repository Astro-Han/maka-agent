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

use super::{
    CallError, FrameError, Progress,
    chunks::Chunks,
    state::{Inner, Stage},
};
use maka_runtime::capability::ClientFrame;
use std::sync::Arc;
use uuid::Uuid;

impl Inner {
    pub fn accept(
        self: &Arc<Self>,
        connection: Uuid,
        frame: ClientFrame,
    ) -> Result<(), FrameError> {
        let id = frame.invocation_id().to_owned();
        let mut state = self.active.lock().unwrap_or_else(|e| e.into_inner());
        let Some(invocation) = state.calls.get_mut(&id) else {
            return if state.retired.contains(&id) {
                Ok(())
            } else {
                Err(FrameError("unmatched invocation"))
            };
        };
        if invocation.registration.connection_id() != connection {
            return Err(FrameError("invocation belongs to another connection"));
        }
        if self.cancelled(&mut state, &id) {
            return Ok(());
        }
        let invocation = state.calls.get_mut(&id).expect("checked active invocation");
        if invocation.pending_terminal.is_some() {
            return Ok(());
        }
        let outcome = match frame {
            ClientFrame::Accepted {
                admission_evidence, ..
            } => {
                if !matches!(invocation.stage, Stage::Dispatched(_)) {
                    return Err(FrameError("repeated acceptance"));
                }
                let Stage::Dispatched(accepted) =
                    std::mem::replace(&mut invocation.stage, Stage::Accepted)
                else {
                    unreachable!()
                };
                invocation.deadline.send_replace(None);
                let _ = accepted.send(Ok(admission_evidence));
                return Ok(());
            }
            ClientFrame::Rejected { message, .. } => {
                if !matches!(invocation.stage, Stage::Dispatched(_)) {
                    return Err(FrameError("rejection after acceptance"));
                }
                Err(CallError::ProviderRejected(message))
            }
            ClientFrame::Failed { message, .. } => {
                if !invocation.stage.admitted() {
                    return Err(FrameError("failure before admission"));
                }
                Err(CallError::ProviderFailed(message))
            }
            ClientFrame::Progress { current, total, .. } => {
                if !matches!(invocation.stage, Stage::Admitted | Stage::Receiving(_)) {
                    return Err(FrameError("progress outside execution phase"));
                }
                if invocation
                    .progress
                    .borrow()
                    .is_some_and(|p| total != p.total || current < p.current)
                {
                    return Err(FrameError("progress regressed or total changed"));
                }
                invocation.progress.send_if_modified(|last| {
                    let next = Some(Progress { current, total });
                    if *last == next {
                        false
                    } else {
                        *last = next;
                        true
                    }
                });
                return Ok(());
            }
            ClientFrame::Result { result, .. } => {
                if !matches!(invocation.stage, Stage::Admitted) {
                    return Err(FrameError("result outside admitted phase"));
                }
                Ok(result)
            }
            ClientFrame::ResultStart {
                byte_length,
                chunk_count,
                ..
            } => {
                if !matches!(invocation.stage, Stage::Admitted) {
                    return Err(FrameError("chunks outside admitted phase"));
                }
                invocation.stage = Stage::Receiving(Chunks::new(byte_length, chunk_count)?);
                return Ok(());
            }
            ClientFrame::ResultChunk { index, data, .. } => {
                let Stage::Receiving(chunks) = &mut invocation.stage else {
                    return Err(FrameError("unexpected result chunk"));
                };
                let Some(result) = chunks.push(index, &data)? else {
                    return Ok(());
                };
                Ok(result)
            }
            ClientFrame::InteractionRequest {
                interaction_id,
                request,
                ..
            } => {
                if !matches!(invocation.stage, Stage::Admitted) {
                    return Err(FrameError("interaction outside admitted phase"));
                }
                self.start_form(&mut state, &id, interaction_id, request);
                return Ok(());
            }
        };
        Self::settle(&mut state, &id, outcome, true);
        Ok(())
    }
}
