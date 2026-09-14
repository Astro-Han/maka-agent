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

use crate::Error;
use maka_runtime::capability::HostFrame;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// One bounded reverse channel. Overflow closes the connection instead of
/// silently losing a release or allowing unbounded peer-owned memory.
#[derive(Clone)]
pub struct Endpoint {
    sender: mpsc::Sender<HostFrame>,
    closed: CancellationToken,
    invocations: CancellationToken,
}

impl Endpoint {
    pub fn channel(capacity: usize) -> (Self, mpsc::Receiver<HostFrame>) {
        Self::channel_with_cancellation(capacity, CancellationToken::new())
    }

    /// Transport cancellation also revokes pending provider invocations, even
    /// while the connection is completing an admitted ordinary request.
    pub fn channel_with_cancellation(
        capacity: usize,
        closed: CancellationToken,
    ) -> (Self, mpsc::Receiver<HostFrame>) {
        let (sender, receiver) = mpsc::channel(capacity);
        let invocations = closed.child_token();
        (
            Self {
                sender,
                closed,
                invocations,
            },
            receiver,
        )
    }

    pub fn closed(&self) -> CancellationToken {
        self.closed.clone()
    }

    /// Supersession cancels old invocations, not the old control connection.
    pub fn invocations(&self) -> CancellationToken {
        self.invocations.clone()
    }

    pub fn send(&self, frame: HostFrame) -> Result<(), Error> {
        if self.closed.is_cancelled() {
            return Err(Error::Unavailable);
        }
        self.sender.try_send(frame).map_err(|_| {
            self.close();
            Error::Unavailable
        })
    }

    pub fn close(&self) {
        self.closed.cancel();
        self.invocations.cancel();
    }

    pub(crate) fn supersede(&self) {
        self.invocations.cancel();
    }
}
