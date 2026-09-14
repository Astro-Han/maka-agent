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

use maka_runtime_host::server::Host;
use maka_transport::{MessageReader, MessageWriter, TransportError};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::sync::mpsc;

struct Reader(mpsc::UnboundedReceiver<Value>);
impl MessageReader for Reader {
    async fn read(&mut self) -> Result<Option<Value>, TransportError> {
        Ok(self.0.recv().await)
    }
}
struct Writer(mpsc::UnboundedSender<Value>);
impl MessageWriter for Writer {
    async fn write(&mut self, value: &Value) -> Result<(), TransportError> {
        self.0
            .send(value.clone())
            .map_err(|_| TransportError::Closed)
    }
    async fn close_after_flush(&mut self) -> Result<(), TransportError> {
        Ok(())
    }
}
pub struct Peer {
    send: mpsc::UnboundedSender<Value>,
    receive: mpsc::UnboundedReceiver<Value>,
    task: tokio::task::JoinHandle<()>,
}
impl Peer {
    pub async fn new(host: Arc<Host>, id: &str) -> Self {
        let (send, reader) = mpsc::unbounded_channel();
        let (writer, receive) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            host.local_owner_connection(Reader(reader), Writer(writer))
                .await
                .unwrap();
        });
        let mut peer = Self {
            send,
            receive,
            task,
        };
        peer.send
            .send(
                json!({"kind":"hello","clientInstanceId":id,"surface":"desktop",
            "activitySnapshotVersion":2,"protocolMin":0,"protocolMax":0,
            "compatibilityEpoch":maka_protocol::COMPATIBILITY_EPOCH,"compositionId":"maka.interactive"}),
            )
            .unwrap();
        assert_eq!(peer.frame().await["state"], "ready");
        peer
    }
    async fn frame(&mut self) -> Value {
        tokio::time::timeout(Duration::from_secs(5), self.receive.recv())
            .await
            .unwrap()
            .expect("Host closed before response")
    }
    pub async fn rpc(&mut self, operation: &str, input: Value) -> Value {
        self.send
            .send(json!({"requestId":operation,"operation":operation,"input":input}))
            .unwrap();
        loop {
            let value = self.frame().await;
            if value["requestId"] == operation {
                return value;
            }
            assert!(value.get("requestId").is_none(), "{value}");
        }
    }
    pub async fn close(self) {
        drop(self.send);
        self.task.await.unwrap();
    }
}
