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

use super::{Lane, Outbound};

impl Outbound {
    pub fn pty_ready(&self, subscription: &str) -> bool {
        !self
            .0
            .state
            .lock()
            .unwrap()
            .pty_pending
            .contains(subscription)
    }

    pub async fn pty_flushed(&self) {
        self.0.pty_flushed.notified().await;
    }

    /// Retire unsent bytes for removed interests; a started native write is not
    /// cancelled or retried. At most one frame per subscription can be pending.
    pub fn retain_pty(&self, subscription: &str, refs: &[String]) {
        let mut state = self.0.state.lock().unwrap();
        let mut removed_bytes = 0;
        let mut removed_frames = 0;
        state.queue.retain(|frame| {
            let remove = frame.lane == Lane::Pty
                && frame.value["subscriptionId"].as_str() == Some(subscription)
                && !refs
                    .iter()
                    .any(|reference| frame.value["ref"].as_str() == Some(reference));
            if remove {
                removed_bytes += frame.bytes;
                removed_frames += 1;
            }
            !remove
        });
        state.bytes -= removed_bytes;
        state.frames -= removed_frames;
        if removed_frames != 0 {
            state.pty_pending.remove(subscription);
            self.0.pty_flushed.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maka_transport::MessageWriter;
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};
    use tokio_util::sync::CancellationToken;

    struct Writer {
        frames: Arc<Mutex<Vec<Value>>>,
        started: CancellationToken,
        released: CancellationToken,
    }
    impl MessageWriter for Writer {
        async fn write(&mut self, value: &Value) -> maka_transport::Result<()> {
            if self.frames.lock().unwrap().is_empty() {
                self.started.cancel();
                self.released.cancelled().await;
            }
            self.frames.lock().unwrap().push(value.clone());
            Ok(())
        }
        async fn close_after_flush(&mut self) -> maka_transport::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn removing_interest_discards_queued_bytes_but_never_cancels_a_started_write() {
        for started_pty in [false, true] {
            let outbound = Outbound::default();
            let frames = Arc::new(Mutex::new(Vec::new()));
            let started = CancellationToken::new();
            let released = CancellationToken::new();
            let closed = CancellationToken::new();
            let pty = json!({"kind":"subscription.runtime_resource_pty_data", "subscriptionId":"s", "ref":"r", "data":"字节"});
            let writer = Writer {
                frames: frames.clone(),
                started: started.clone(),
                released: released.clone(),
            };
            let publish = async {
                outbound
                    .enqueue(if started_pty {
                        pty.clone()
                    } else {
                        json!({"kind":"control"})
                    })
                    .await
                    .unwrap();
                started.cancelled().await;
                if !started_pty {
                    outbound.enqueue(pty).await.unwrap();
                }
                assert!(!outbound.pty_ready("s"));
                outbound.retain_pty("s", &[]);
                assert_eq!(outbound.pty_ready("s"), !started_pty);
                outbound
                    .enqueue(json!({"kind":"control_after_removal"}))
                    .await
                    .unwrap();
                released.cancel();
                outbound.close();
            };
            let (_, delivered) = tokio::join!(publish, outbound.run(writer, &closed));
            delivered.unwrap();
            assert!(outbound.pty_ready("s"));
            let frames = frames.lock().unwrap();
            assert_eq!(frames.len(), 2);
            assert_eq!(
                frames[0]["kind"],
                if started_pty {
                    "subscription.runtime_resource_pty_data"
                } else {
                    "control"
                }
            );
            assert_eq!(frames[1]["kind"], "control_after_removal");
        }
    }
}
