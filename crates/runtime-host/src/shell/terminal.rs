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
    Result, ShellError,
    control::{ControlError, WriteReceipt},
    output::Output,
};
use maka_js_runtime::terminal::Screen;
use maka_process::pty::PtyIo;
use maka_runtime::terminal::{TerminalScreen, TerminalSize};
use std::collections::VecDeque;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub(super) struct Terminal {
    screen: Option<Screen>,
    pub snapshot: TerminalScreen,
    decoder: Decoder,
    pub writes: VecDeque<PendingWrite>,
    output: Output,
}
impl Terminal {
    pub async fn new(
        runtime: maka_js_runtime::trusted::TrustedRuntime,
        size: TerminalSize,
        output: Output,
    ) -> Result<Self> {
        let mut screen = Screen::with_runtime(runtime, size)?;
        let snapshot = match screen.snapshot().await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                screen.close().await;
                return Err(error.into());
            }
        };
        Ok(Self {
            screen: Some(screen),
            snapshot,
            decoder: Decoder::default(),
            writes: VecDeque::new(),
            output,
        })
    }

    pub async fn output(&mut self, bytes: &[u8], eof: bool) -> Result<()> {
        let text = self.decoder.decode(bytes, eof);
        let Some(screen) = &mut self.screen else {
            return Ok(());
        };
        let result = async {
            let reply = screen.write(&text).await?;
            let snapshot = screen.snapshot().await?;
            Ok::<_, maka_js_runtime::terminal::ScreenError>((reply, snapshot))
        }
        .await;
        match result {
            Ok((reply, snapshot)) => {
                self.snapshot = snapshot;
                // Observational stream, not a durable outcome. Publish only a
                // completed parser cut, including final output during drain.
                self.output.publish(&text)?;
                if !reply.is_empty() {
                    self.writes
                        .push_back(PendingWrite::new(reply, None, false, false));
                }
                Ok(())
            }
            Err(error) => {
                // Last good cut survives. Await disposal before the native
                // worker can finish and release resource residency.
                self.close().await;
                Err(error.into())
            }
        }
    }

    pub async fn resize(&mut self, size: TerminalSize) -> Result<()> {
        let screen = self
            .screen
            .as_mut()
            .ok_or(ShellError::Rejected("terminal parser unavailable"))?;
        screen.resize(size).await?;
        self.snapshot = screen.snapshot().await?;
        self.output.resize(size);
        Ok(())
    }

    pub fn fail_writes(&mut self, message: &str) {
        for mut pending in self.writes.drain(..) {
            if let Some(reply) = pending.reply.take() {
                let _ = reply.send(Err(ControlError::new(message, pending.accepted)
                    .after_resize(pending.resized, pending.resize_changed)));
            }
        }
    }

    pub async fn close(&mut self) {
        if let Some(screen) = self.screen.take() {
            screen.close().await;
        }
    }
}

pub(super) struct PendingWrite {
    bytes: Vec<u8>,
    pub accepted: usize,
    reply: Option<oneshot::Sender<std::result::Result<WriteReceipt, ControlError>>>,
    resized: bool,
    resize_changed: bool,
}
impl PendingWrite {
    pub fn new(
        text: String,
        reply: Option<oneshot::Sender<std::result::Result<WriteReceipt, ControlError>>>,
        resized: bool,
        resize_changed: bool,
    ) -> Self {
        Self {
            bytes: text.into_bytes(),
            accepted: 0,
            reply,
            resized,
            resize_changed,
        }
    }
    pub fn remaining(&self) -> &[u8] {
        &self.bytes[self.accepted..]
    }
    pub fn advance(&mut self, count: usize) -> bool {
        self.accepted += count;
        self.accepted == self.bytes.len()
    }

    pub fn finish(mut self, record: std::sync::Arc<maka_runtime::shell_run::ShellRun>) {
        if let Some(reply) = self.reply.take() {
            let _ = reply.send(Ok(WriteReceipt {
                accepted_bytes: self.accepted,
                resized: self.resized,
                resize_changed: self.resize_changed,
                record,
            }));
        }
    }
}

/// Retains only the incomplete UTF-8 suffix (at most three bytes). Malformed
/// bytes and a genuinely incomplete EOF use replacement characters, like xterm.
#[derive(Default)]
pub(super) struct Decoder {
    pending: Vec<u8>,
}
impl Decoder {
    pub(super) fn decode(&mut self, bytes: &[u8], eof: bool) -> String {
        self.pending.extend_from_slice(bytes);
        let mut text = String::new();
        let mut consumed = 0;
        while consumed < self.pending.len() {
            match std::str::from_utf8(&self.pending[consumed..]) {
                Ok(valid) => {
                    text.push_str(valid);
                    consumed = self.pending.len();
                }
                Err(error) => {
                    let end = consumed + error.valid_up_to();
                    text.push_str(std::str::from_utf8(&self.pending[consumed..end]).unwrap());
                    consumed = end;
                    match error.error_len() {
                        Some(length) => {
                            text.push('\u{fffd}');
                            consumed += length;
                        }
                        None if eof => {
                            text.push('\u{fffd}');
                            consumed = self.pending.len();
                        }
                        None => break,
                    }
                }
            }
        }
        self.pending.drain(..consumed);
        text
    }
}

pub(super) async fn drain(
    terminal: &mut Terminal,
    io: &PtyIo,
    exited: &CancellationToken,
) -> Result<()> {
    let deadline = async {
        exited.cancelled().await;
        tokio::time::sleep(Duration::from_secs(2)).await;
    };
    tokio::pin!(deadline);
    let mut buffer = [0; 16 * 1024];
    let mut failure = None;
    loop {
        let count = tokio::select! {
            _ = &mut deadline => {
                io.discard_output()?;
                terminal.snapshot.truncated = true;
                return Err(ShellError::Rejected("PTY output did not drain after root exit"));
            }
            read = io.read(&mut buffer) => match read {
                Ok(count) => count,
                Err(error) => { io.discard_output()?; return Err(error.into()); }
            }
        };
        if let Err(error) = terminal.output(&buffer[..count], count == 0).await {
            failure.get_or_insert(error);
        }
        // After root exit replies have no consumer and are not input effects.
        terminal.fail_writes("PTY root exited");
        if count == 0 {
            return failure.map_or(Ok(()), Err);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Decoder;

    #[test]
    fn every_utf8_cut_matches_lossy_eof_without_losing_terminal_controls() {
        let bytes = b"\x1b[31m\xe4\xb8\xad\xf0\x9f\x98\x80\xff\xe2\x82";
        for first in 0..=bytes.len() {
            for second in first..=bytes.len() {
                let mut decoder = Decoder::default();
                let actual = decoder.decode(&bytes[..first], false)
                    + &decoder.decode(&bytes[first..second], false)
                    + &decoder.decode(&bytes[second..], true);
                assert_eq!(actual, String::from_utf8_lossy(bytes));
                assert!(decoder.pending.is_empty());
            }
        }
    }
}
