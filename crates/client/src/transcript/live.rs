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
use maka_protocol::subscription::SessionAssistantDelta;

/// A derived live stream, not durable history. Offsets on the wire are UTF-16.
#[derive(Default)]
pub struct LiveText {
    pub text: String,
    pub complete: bool,
    pub interrupted: bool,
    units: u64,
    run_id: Option<String>,
}
impl LiveText {
    pub fn apply(&mut self, delta: &SessionAssistantDelta) -> Result<(), Error> {
        if self.run_id.as_ref().is_some_and(|id| *id != delta.run_id) {
            return Err("Assistant stream run changed".into());
        }
        let reset = delta.reset.is_some();
        let expected = if reset { 0 } else { self.units };
        let bytes = if reset { 0 } else { self.text.len() };
        if delta.start_offset != expected || self.complete && !reset {
            return Err("Assistant stream offset changed or continued after completion".into());
        }
        if bytes + delta.text.len() > super::MESSAGE_BYTES {
            return Err("Assistant stream exceeds local byte limit".into());
        }
        if reset {
            self.text.clear();
        }
        self.text.push_str(&delta.text);
        self.units = expected + delta.text.encode_utf16().count() as u64;
        self.complete = delta.complete.is_some();
        self.interrupted = delta.interrupted.is_some();
        self.run_id = Some(delta.run_id.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maka_protocol::subscription::{AssistantStreamKind, TrueFlag};
    #[test]
    fn offsets_count_utf16_and_reset_does_not_duplicate_replayed_text() {
        let mut stream = LiveText::default();
        let mut delta = SessionAssistantDelta {
            kind: AssistantStreamKind::Text,
            turn_id: "t".into(),
            run_id: "r".into(),
            message_id: "m".into(),
            start_offset: 0,
            text: "中🦀é".into(),
            reset: None,
            complete: None,
            interrupted: None,
        };
        stream.apply(&delta).unwrap();
        delta.start_offset = 4;
        delta.text = "!".into();
        assert!(stream.apply(&delta).is_err());
        delta.start_offset = 5;
        stream.apply(&delta).unwrap();
        assert_eq!(stream.text, "中🦀é!");
        delta.reset = Some(TrueFlag);
        delta.start_offset = 0;
        delta.text = "replay".into();
        delta.complete = Some(TrueFlag);
        delta.interrupted = Some(TrueFlag);
        stream.apply(&delta).unwrap();
        assert_eq!(stream.text, "replay");
        assert!(stream.complete && stream.interrupted);
        delta.reset = None;
        delta.start_offset = 6;
        assert!(stream.apply(&delta).is_err());
    }
}
