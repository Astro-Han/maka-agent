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

use super::Worker;
use crate::shell::{
    Result,
    control::{Control, ControlError, Input},
    terminal::{PendingWrite, Terminal},
};
use maka_process::pty::PtyChild;
use maka_runtime::{shell_run::ShellRun, terminal::input::encode_actions};
use std::sync::Arc;

impl Worker {
    pub(super) async fn control(
        &mut self,
        terminal: &mut Terminal,
        child: &mut PtyChild,
        control: Control,
    ) -> Result<()> {
        if control.cancellation.is_cancelled() {
            let _ = control.reply.send(Err(ControlError::rejected(
                "PTY control cancelled before effect",
            )));
            return Ok(());
        }
        let bytes = match control.input {
            Input::Raw(bytes) => bytes,
            Input::Actions(actions) => {
                match encode_actions(
                    &actions,
                    terminal.snapshot.input,
                    control.size.unwrap_or(terminal.snapshot.size),
                ) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        let _ = control.reply.send(Err(ControlError::rejected(error)));
                        return Ok(());
                    }
                }
            }
        };
        let mut resized = false;
        let mut resize_changed = false;
        if let Some(size) = control.size {
            let result = async {
                if size != terminal.snapshot.size {
                    child.resize(size)?;
                    resize_changed = true;
                }
                resized = true;
                if resize_changed {
                    terminal.resize(size).await?;
                }
                self.persist(terminal, None).await
            }
            .await;
            if let Err(error) = result {
                let _ = control.reply.send(Err(
                    ControlError::new(&error, 0).after_resize(resized, resize_changed)
                ));
                return Err(error);
            }
        }
        if bytes.is_empty() {
            let _ = control.reply.send(Ok(crate::shell::WriteReceipt {
                accepted_bytes: 0,
                resized,
                resize_changed,
                record: self.current_cut(),
            }));
        } else {
            terminal.writes.push_back(PendingWrite::new(
                bytes,
                Some(control.reply),
                resized,
                resize_changed,
            ));
        }
        Ok(())
    }

    pub(super) fn current_cut(&self) -> Arc<ShellRun> {
        // This sole worker awaits every parser/SQL cut before servicing input.
        // Cloning the published Arc does not create another snapshot authority.
        self.updates
            .borrow()
            .as_ref()
            .expect("running worker published a cut")
            .as_ref()
            .expect("running worker has no terminal error")
            .clone()
    }
}
