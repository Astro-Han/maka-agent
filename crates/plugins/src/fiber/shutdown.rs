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

use super::{Inner, Phase};
use std::sync::Arc;

impl Inner {
    pub(super) fn retire(self: &Arc<Self>) {
        let (mut effects, children) = {
            let mut state = self.state.lock().unwrap();
            if matches!(
                state.phase,
                Phase::Unloading | Phase::Failed | Phase::Disposed
            ) {
                return;
            }
            state.phase = Phase::Unloading;
            state.effective = false;
            self.changed.send_replace(());
            (
                std::mem::take(&mut state.effects)
                    .into_values()
                    .collect::<Vec<_>>(),
                std::mem::take(&mut state.children),
            )
        };
        self.stopping.cancel();
        // Notify every owner before waiting; a close callback can depend on a
        // process or child that was registered earlier than the callback itself.
        for child in &children {
            child.retire();
        }
        let mut failures = Vec::new();
        for effect in effects.iter_mut().rev() {
            if let Err(error) = effect.signal() {
                failures.push(error);
            }
        }
        let inner = self.clone();
        self.runtime.spawn(async move {
            for child in children.into_iter().rev() {
                let mut changed = child.inner.changed.subscribe();
                loop {
                    {
                        let state = child.inner.state.lock().unwrap();
                        if matches!(state.phase, Phase::Failed | Phase::Disposed) {
                            failures.extend(state.failures.clone());
                            break;
                        }
                    }
                    if changed.changed().await.is_err() {
                        failures.push("child cleanup owner disappeared".into());
                        break;
                    }
                }
            }
            loop {
                let settled = inner.settled.notified();
                if inner.state.lock().unwrap().calls == 0 {
                    break;
                }
                settled.await;
            }
            for effect in effects.into_iter().rev() {
                if let Err(error) = effect.close().await {
                    failures.push(error);
                }
            }
            let mut state = inner.state.lock().unwrap();
            failures.append(&mut state.failures);
            state.phase = if failures.is_empty() {
                Phase::Disposed
            } else {
                Phase::Failed
            };
            state.failures = failures;
            inner.changed.send_replace(());
        });
    }
}
