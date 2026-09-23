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

/// The shared process Screen intentionally flushes synchronized output at parser
/// cuts for persistence/replies. UI tests instead wait for the renderer's end
/// marker before inspecting it, regardless of PTY read boundaries.
#[derive(Default, Debug)]
pub(super) struct Frames {
    tail: [u8; 8],
    drawing: bool,
    complete: bool,
    resizing: bool,
    cleared: bool,
}

impl Frames {
    pub fn resize(&mut self) {
        self.resizing = true;
        self.cleared = false;
    }

    pub fn ready(&self) -> bool {
        self.complete && !self.drawing && !self.resizing
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.tail.copy_within(1.., 0);
            self.tail[7] = byte;
            if &self.tail == b"\x1b[?2026h" {
                self.drawing = true;
                self.complete = false;
                self.cleared = false;
            } else if self.drawing && self.tail.ends_with(b"\x1b[2J") {
                // Ratatui clears on actual viewport resize. Emulator reflow of
                // the old screen alone must not satisfy a resized-layout wait.
                self.cleared = true;
            } else if &self.tail == b"\x1b[?2026l" && self.drawing {
                self.drawing = false;
                self.complete = true;
                if self.cleared {
                    self.resizing = false;
                }
            }
        }
    }
}

#[test]
fn partial_frames_and_reflow_never_publish_click_coordinates() {
    let mut frames = Frames::default();
    for byte in b"\x1b[?2026hWorkspace\x1b[?2026" {
        frames.feed(&[*byte]);
        assert!(!frames.ready());
    }
    frames.feed(b"l");
    assert!(frames.ready());
    frames.resize();
    frames.feed(b"\x1b[?2026hqueued old frame\x1b[?2026l");
    assert!(!frames.ready());
    frames.feed(b"\x1b[?2026h\x1b[2Jresized frame");
    assert!(!frames.ready());
    frames.feed(b"\x1b[?2026l\x1b[?2026hnext partial frame");
    assert!(!frames.ready());
    frames.feed(b"\x1b[?2026l");
    assert!(frames.ready());
}
