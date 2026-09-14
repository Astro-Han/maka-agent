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

#![cfg(unix)]

use super::live_shell::{command, record, setup, wait_text};
use maka_runtime::{
    shell_run::{ShellOutcome, ShellState},
    terminal::TerminalSize,
};
use maka_runtime_host::shell::{PtyStreamEvent, ShellResources};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn raw_controls_resize_before_input_and_stream_cuts_recover_after_lag() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let temp = tempfile::tempdir().unwrap();
        let log = setup(&temp.path().join("runtime.sqlite")).await;
        let drain = CancellationToken::new();
        let resources = ShellResources::new(log.clone(), drain.clone());
        let script = r#"
            stty raw -echo
            printf 'ready-中'
            input=$(dd bs=1 count=8 2>/dev/null | od -An -tx1 | tr -d ' \n')
            test "$input" = 1b5b4100e4b8ad0d || exit 4
            test "$(stty size)" = '30 100' || exit 5
            printf '\r\naccepted-中'
            dd bs=1 count=1 >/dev/null 2>&1
            i=0; while test "$i" -lt 16384; do printf 'tail-%s\r\n' "$i"; i=$((i+1)); done
            printf 'final-中'
        "#;
        let size = TerminalSize::new(80, 24).unwrap();
        let mut handle = resources
            .start_pty(
                record(temp.path(), "raw-stream", script),
                command(temp.path(), script, ""),
                size,
            )
            .unwrap();
        wait_text(&mut handle, "ready-中").await;
        let (cut, mut stream) = handle.attach().unwrap();
        assert!(cut.buffer.contains("ready-中"));
        let resized = TerminalSize::new(100, 30).unwrap();
        let rejected = handle
            .write_raw("x".repeat(64 * 1024 + 1), Some(resized))
            .await
            .unwrap_err();
        assert_eq!(rejected.accepted_bytes, Some(0));
        assert_eq!(rejected.resized, Some(false));
        assert_eq!(
            handle.replay().unwrap().size,
            size,
            "reject the entire combined operation before resize"
        );

        let receipt = handle
            .write_raw("\x1b[A\0中\r".into(), Some(resized))
            .await
            .unwrap();
        assert_eq!(receipt.accepted_bytes, 8);
        assert!(receipt.resized);
        assert_eq!(handle.replay().unwrap().size, resized);
        let (mut sequence, mut seen) = (cut.sequence, cut.buffer);
        while !seen.contains("accepted-中") {
            let PtyStreamEvent::Data(frame) = stream.next().await else {
                panic!("unexpected gap");
            };
            sequence += 1;
            assert_eq!(frame.sequence, sequence);
            seen.push_str(&frame.data);
        }
        let (later_cut, mut lagged) = handle.attach().unwrap();
        assert!(later_cut.sequence >= sequence);
        assert!(later_cut.buffer.contains("accepted-中"));
        assert_eq!(later_cut.size, resized);
        handle.write_raw("!".into(), None).await.unwrap();
        let terminal = handle.finished().await.unwrap();
        assert!(matches!(
            terminal.state,
            ShellState::Terminal {
                outcome: ShellOutcome::Completed,
                ..
            }
        ));
        resources.shutdown().await;
        // Deliberately do not consume the burst. Recovery is a single coherent
        // cut, followed by EOF; not a silent gap or an unbounded buffered history.
        let PtyStreamEvent::Reset(last) = lagged.next().await else {
            panic!("expected bounded-ring reset");
        };
        assert!(last.sequence > later_cut.sequence + 8);
        assert!(last.buffer.ends_with("final-中"));
        assert!(last.buffer.len() <= 80 * 1024);
        assert!(matches!(lagged.next().await, PtyStreamEvent::Closed));
        assert_eq!(
            *terminal,
            log.read_shell_run("session", "raw-stream")
                .await
                .unwrap()
                .unwrap()
        );
        assert_eq!(resources.active_count(), 0);
        assert!(!drain.is_cancelled());
        log.shutdown().await.unwrap();
    })
    .await
    .unwrap();
}
