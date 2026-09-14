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

use crate::ModelEvent;
use serde_json::Value;

/// Bound individual journal observations independently of provider chunk sizes.
/// Splitting is linear and retains provider metadata once, on the first chunk.
pub(crate) struct PendingDelta {
    id: String,
    text: String,
    offset: usize,
    provider_options: Option<Value>,
}

impl PendingDelta {
    pub fn new(id: String, text: String, provider_options: Option<Value>) -> Self {
        Self {
            id,
            text,
            offset: 0,
            provider_options,
        }
    }
}

impl Iterator for PendingDelta {
    type Item = ModelEvent;

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset == self.text.len() {
            return None;
        }
        let mut end = (self.offset + 8 * 1024).min(self.text.len());
        while !self.text.is_char_boundary(end) {
            end -= 1;
        }
        let text = self.text[self.offset..end].to_owned();
        self.offset = end;
        Some(ModelEvent::PartDelta {
            id: self.id.clone(),
            text,
            provider_options: self.provider_options.take(),
        })
    }
}
