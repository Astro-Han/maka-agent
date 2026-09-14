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

use maka_js_runtime::terminal::Screen;
use maka_runtime::terminal::TerminalSize;
use serde_json::{Value, json};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

#[cfg(unix)]
#[tokio::test]
async fn real_pty_queries_and_mode_aware_input_cross_the_screen_cut() {
    use maka_process::pty;
    use maka_runtime::terminal::input::{InputAction, encode_actions};

    let expected = "\u{1b}OA中\u{1b}[<0;2;2M\u{1b}[<0;2;2m\r";
    let hex = expected
        .bytes()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let mut command = pty::PtyCommand::new("/bin/sh", std::env::current_dir().unwrap());
    command.args([
        "-c",
        &format!(
            r#"
        stty raw -echo
        printf '\033[6n'
        reply=$(dd bs=1 count=6 2>/dev/null | od -An -tx1 | tr -d ' \n')
        test "$reply" = 1b5b313b3152 || exit 3
        printf '\033[?1h\033[?1006h\033[?1002hready'
        input=$(dd bs=1 count={} 2>/dev/null | od -An -tx1 | tr -d ' \n')
        test "$input" = {hex} || exit 4
        printf '\r\nverified'
        exit 42
    "#,
            expected.len()
        ),
    ]);
    let size = TerminalSize::new(80, 24).unwrap();
    let (mut child, io) = pty::spawn(command, size).await.unwrap();
    let mut screen = Screen::new(size).unwrap();
    let actions = json!([
        {"type":"key","key":"arrow_up"},
        {"type":"text","text":"中"},
        {"type":"mouse","event":"click","button":"left","x":1,"y":1},
        {"type":"key","key":"enter"}
    ])
    .as_array()
    .unwrap()
    .iter()
    .cloned()
    .map(InputAction::parse)
    .collect::<Result<Vec<_>, _>>()
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut sent = false;
        let mut bytes = [0; 1024];
        loop {
            let count = io.read(&mut bytes).await.unwrap();
            if count == 0 {
                break;
            }
            // This fixture emits ASCII; native UTF-8 input is checked by the
            // child byte-for-byte, independent of read chunk boundaries.
            let replies = screen
                .write(std::str::from_utf8(&bytes[..count]).unwrap())
                .await
                .unwrap();
            write_pty(&io, replies.as_bytes()).await;
            let cut = screen.snapshot().await.unwrap();
            if !sent && cut.screen.contains("ready") {
                let input = encode_actions(&actions, cut.input, cut.size).unwrap();
                assert_eq!(input, expected);
                write_pty(&io, input.as_bytes()).await;
                sent = true;
            }
        }
        assert!(sent);
        assert_eq!(child.wait().await.unwrap().code(), Some(42));
        assert!(screen.snapshot().await.unwrap().screen.contains("verified"));
    })
    .await
    .unwrap();

    async fn write_pty(io: &pty::PtyIo, mut input: &[u8]) {
        while !input.is_empty() {
            let count = io.write(input).await.unwrap();
            assert!(count > 0);
            input = &input[count..];
        }
    }
}

#[tokio::test]
async fn screen_matches_original_collector_across_parser_cuts_modes_and_reflow() {
    let actions = json!([
        {"write":"line\r\n".repeat(502)},
        {"write":"\u{1b}[?1049l"},
        {"write":"overflow\r\n"},
        {"write":"\u{1b}c"},
        {"write":"hello 中文😀\r\nnext\u{1b}[6n"},
        {"cols":5,"rows":3},
        {"write":"\r\n\u{1b}[?1h\u{1b}[?1002h\u{1b}[?1006h\u{1b}[?25l"},
        {"write":"\u{1b}[?1049hALT 中文\r\nsecond"},
        {"write":"\u{1b}[?1049l\u{1b}[?1016h"},
        {"write":"\u{1b}]52;c;ignored"},
        {"write":"\u{7}OK\u{1b}[5n\u{1b}[6n"},
        {"write":"\r\n0123456789\r\n".repeat(520)},
        {"cols":20,"rows":5},
        {"write":"\u{1b}creset\u{1b}[6n"}
    ]);
    let mut screen = Screen::new(TerminalSize::new(10, 3).unwrap()).unwrap();
    let mut observations = vec![];
    for action in actions.as_array().unwrap() {
        let replies = if let Some(data) = action["write"].as_str() {
            screen.write(data).await.unwrap()
        } else {
            screen
                .resize(
                    TerminalSize::new(
                        action["cols"].as_u64().unwrap() as u16,
                        action["rows"].as_u64().unwrap() as u16,
                    )
                    .unwrap(),
                )
                .await
                .unwrap();
            String::new()
        };
        observations.push(json!({"screen":screen.snapshot().await.unwrap(),"replies":replies}));
    }
    let oracle = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/terminal-screen-oracle.mjs");
    let mut child = Command::new("node")
        .arg(oracle)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&actions).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let expected: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    for (index, (actual, expected)) in observations.iter().zip(&expected).enumerate() {
        assert_eq!(actual, expected, "parser cut {index}");
    }
    assert_eq!(observations.len(), expected.len());
}

#[tokio::test]
async fn gaps_reset_escape_state_and_incomplete_cuts_cannot_be_reused() {
    let size = TerminalSize::new(10, 3).unwrap();
    let mut screen = Screen::new(size).unwrap();
    screen
        .write("before\u{1b}]52;c;unterminated")
        .await
        .unwrap();
    screen.reset_after_gap().await.unwrap();
    screen.write("after").await.unwrap();
    let snapshot = screen.snapshot().await.unwrap();
    assert_eq!(snapshot.screen, "after");
    assert!(snapshot.truncated);
    assert!(screen.write(&"x".repeat(65537)).await.is_err());
    assert_eq!(
        screen.snapshot().await.unwrap(),
        snapshot,
        "pre-admission rejection has no effect"
    );

    let mut interrupted = Screen::new(size).unwrap();
    {
        let write = interrupted.write("unfinished");
        tokio::pin!(write);
        assert!(
            std::future::poll_fn(|cx| match write.as_mut().poll(cx) {
                std::task::Poll::Pending => std::task::Poll::Ready(true),
                std::task::Poll::Ready(_) => std::task::Poll::Ready(false),
            })
            .await
        );
    }
    assert!(interrupted.snapshot().await.is_err());

    let mut flooded = Screen::new(size).unwrap();
    let queries = "\u{1b}[6n".repeat(16384);
    let outcome = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if flooded.write(&queries).await.is_err() {
                break;
            }
        }
    })
    .await;
    assert!(
        outcome.is_ok(),
        "reply flooding must fail within its lifetime budget"
    );
    assert!(flooded.snapshot().await.is_err());

    // Reject short escape sequences requesting huge work before their handlers
    // allocate backing memory or enter loops proportional to the parameter.
    for final_byte in ['I', 'Z', 'L', 'M', 'S', 'T', 'b'] {
        let mut amplified = Screen::new(size).unwrap();
        let result = amplified
            .write(&format!("x\u{1b}[2147483647{final_byte}"))
            .await;
        assert!(result.unwrap_err().to_string().contains("expansion budget"));
        assert!(amplified.snapshot().await.is_err());
    }
}
