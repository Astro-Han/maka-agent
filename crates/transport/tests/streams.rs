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

use futures_util::{SinkExt, StreamExt, poll};
use maka_protocol::{ErrorCode, MAX_MESSAGE_BYTES};
use maka_transport::{TransportError, ndjson, websocket};
use serde_json::json;
use tokio::io::{AsyncWriteExt, duplex};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
        Message,
        protocol::{
            Role,
            frame::{
                Frame,
                coding::{Data, OpCode},
            },
        },
    },
};
use tokio_util::sync::CancellationToken;

fn code(error: TransportError) -> ErrorCode {
    match error {
        TransportError::Protocol(e) => e.code,
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn ndjson_fragmentation_coalescing_crlf_bom_and_partial_eof() {
    // Source oracle: LocalIpcProtocolFrameDecoder.push/end (epoch 141).
    let input = "\u{feff}{\"x\":\"😀\"}\r\n[1,2]\n".as_bytes();
    for chunk_size in [1, 2, 4096] {
        let (mut peer, stream) = duplex(16);
        let (mut reader, _writer) = ndjson::split(stream, CancellationToken::new());
        let produce = async {
            for chunk in input.chunks(chunk_size) {
                peer.write_all(chunk).await.unwrap();
            }
            peer.shutdown().await.unwrap();
        };
        let consume = async {
            assert_eq!(reader.read().await.unwrap(), Some(json!({"x":"😀"})));
            assert_eq!(reader.read().await.unwrap(), Some(json!([1, 2])));
            assert!(reader.read().await.unwrap().is_none());
        };
        tokio::join!(produce, consume);
    }
    for (input, expected) in [
        (&b"{}"[..], ErrorCode::InvalidFrame),
        (&b"\n"[..], ErrorCode::InvalidFrame),
        (&b"\r\n"[..], ErrorCode::InvalidJson),
        (&b"\xff\n"[..], ErrorCode::InvalidUtf8),
    ] {
        let (mut peer, stream) = duplex(16);
        let (mut reader, _writer) = ndjson::split(stream, CancellationToken::new());
        peer.write_all(input).await.unwrap();
        peer.shutdown().await.unwrap();
        assert_eq!(code(reader.read().await.unwrap_err()), expected);
    }
}

#[tokio::test]
async fn ndjson_byte_limit_read_cancel_safety_and_cancelled_write_poisoning() {
    for excess in [0, 1] {
        let (mut peer, stream) = duplex(1024);
        let token = CancellationToken::new();
        let (mut reader, _writer) = ndjson::split(stream, token.clone());
        let body = json!("x".repeat(MAX_MESSAGE_BYTES - 2 + excess));
        let mut bytes = serde_json::to_vec(&body).unwrap();
        bytes.push(b'\n');
        let produce = async {
            let _ = peer.write_all(&bytes).await;
        };
        let consume = async {
            let result = reader.read().await;
            if excess == 0 {
                assert_eq!(result.unwrap(), Some(body));
            } else {
                assert_eq!(code(result.unwrap_err()), ErrorCode::FrameTooLarge);
            }
            drop(reader);
        };
        tokio::join!(produce, consume);
    }
    let (mut peer, stream) = duplex(32);
    let token = CancellationToken::new();
    let (mut reader, mut writer) = ndjson::split(stream, token.clone());
    peer.write_all(b"{\"x\":").await.unwrap();
    {
        let pending = reader.read();
        tokio::pin!(pending);
        assert!(poll!(&mut pending).is_pending());
    }
    assert!(!token.is_cancelled());
    peer.write_all(b"1}\n").await.unwrap();
    assert_eq!(reader.read().await.unwrap(), Some(json!({"x":1})));
    {
        let body = json!("x".repeat(1024));
        let pending = writer.write(&body);
        tokio::pin!(pending);
        assert!(poll!(&mut pending).is_pending()); // Backpressure; peer is not reading.
    }
    assert!(token.is_cancelled()); // Partial write must never be reused.
    assert!(matches!(reader.read().await, Err(TransportError::Closed)));
}

#[tokio::test]
async fn ndjson_flush_close_keeps_all_prior_frames() {
    let (a, b) = duplex(8);
    let (_read_a, mut write_a) = ndjson::split(a, CancellationToken::new());
    let (mut read_b, _write_b) = ndjson::split(b, CancellationToken::new());
    let send = async {
        write_a.write(&json!({"one":1})).await.unwrap();
        write_a.write(&json!({"two":2})).await.unwrap();
        write_a.close_after_flush().await.unwrap();
        assert!(matches!(
            write_a.write(&json!(null)).await,
            Err(TransportError::Closed)
        ));
    };
    let receive = async {
        assert_eq!(read_b.read().await.unwrap(), Some(json!({"one":1})));
        assert_eq!(read_b.read().await.unwrap(), Some(json!({"two":2})));
        assert!(read_b.read().await.unwrap().is_none());
    };
    tokio::join!(send, receive);
}

// Raw sockets are test-only: production callers must authenticate and complete
// their HTTP upgrade before constructing this transport.
async fn ws_pair() -> (
    WebSocketStream<tokio::io::DuplexStream>,
    WebSocketStream<tokio::io::DuplexStream>,
) {
    let (client, server) = duplex(4096);
    tokio::join!(
        WebSocketStream::from_raw_socket(client, Role::Client, Some(websocket::config())),
        WebSocketStream::from_raw_socket(server, Role::Server, Some(websocket::config())),
    )
}

#[tokio::test]
async fn websocket_fragmented_text_ping_and_close_are_driven() {
    let (mut peer, socket) = ws_pair().await;
    let (mut reader, writer) = websocket::split(socket, CancellationToken::new()).unwrap();
    let send = async {
        peer.send(Message::Frame(Frame::message(
            b"{\"x\":".to_vec(),
            OpCode::Data(Data::Text),
            false,
        )))
        .await
        .unwrap();
        peer.send(Message::Ping(vec![1].into())).await.unwrap();
        peer.send(Message::Frame(Frame::message(
            b"1}".to_vec(),
            OpCode::Data(Data::Continue),
            true,
        )))
        .await
        .unwrap();
        peer.close(None).await.unwrap();
        while peer.next().await.is_some() {}
    };
    let receive = async {
        assert_eq!(reader.read().await.unwrap(), Some(json!({"x":1})));
        assert!(reader.read().await.unwrap().is_none());
        drop(reader);
        drop(writer);
    };
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        tokio::join!(send, receive);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn websocket_rejects_binary_empty_oversize_and_invalid_utf8() {
    for (message, expected) in [
        (
            Message::Binary(vec![b'{', b'}'].into()),
            ErrorCode::InvalidFrame,
        ),
        (Message::Text("".into()), ErrorCode::InvalidFrame),
        (
            Message::Text("x".repeat(MAX_MESSAGE_BYTES + 1).into()),
            ErrorCode::FrameTooLarge,
        ),
        (
            Message::Frame(Frame::message(vec![0xff], OpCode::Data(Data::Text), true)),
            ErrorCode::InvalidUtf8,
        ),
    ] {
        let (mut peer, socket) = ws_pair().await;
        let token = CancellationToken::new();
        let (mut reader, writer) = websocket::split(socket, token.clone()).unwrap();
        let send = async {
            let _ = peer.send(message).await;
        };
        let receive = async {
            assert_eq!(code(reader.read().await.unwrap_err()), expected);
            assert!(token.is_cancelled());
            drop(reader);
            drop(writer);
        };
        tokio::join!(send, receive);
    }
}

#[tokio::test]
async fn websocket_writer_flushes_text_then_normal_close_and_cancellation_wakes_read() {
    let (mut peer, socket) = ws_pair().await;
    let token = CancellationToken::new();
    let (mut reader, mut writer) = websocket::split(socket, token.clone()).unwrap();
    let send = async {
        writer.write(&json!({"ok": true})).await.unwrap();
        writer.close_after_flush().await.unwrap();
    };
    let receive = async {
        assert_eq!(
            peer.next().await.unwrap().unwrap(),
            Message::Text("{\"ok\":true}".into())
        );
        match peer.next().await.unwrap().unwrap() {
            Message::Close(Some(frame)) => assert_eq!(u16::from(frame.code), 1000),
            other => panic!("Expected normal close, got {other:?}"),
        }
    };
    tokio::join!(send, receive);
    token.cancel();
    assert!(matches!(reader.read().await, Err(TransportError::Closed)));
}
