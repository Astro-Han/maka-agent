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

use super::{Head, message};
use tokio::sync::{mpsc, oneshot};

pub(super) async fn run(
    request: reqwest::RequestBuilder,
    head: oneshot::Sender<Result<Head, String>>,
    send: mpsc::Sender<Vec<u8>>,
) -> Result<(), String> {
    let mut response = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            let _ = head.send(Err(message(error)));
            return Ok(());
        }
    };
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| (name.as_str().to_owned(), value.as_bytes().to_vec()))
        .collect::<Vec<_>>();
    if headers
        .iter()
        .map(|(name, value)| name.len() + value.len())
        .sum::<usize>()
        > 64 * 1024
    {
        let _ = head.send(Err("HTTP response headers exceed 64 KiB".into()));
        return Ok(());
    }
    if head
        .send(Ok(Head {
            status: response.status().as_u16(),
            url: response.url().to_string(),
            headers,
        }))
        .is_err()
    {
        return Ok(());
    }
    loop {
        match response.chunk().await {
            Ok(Some(bytes)) => {
                for chunk in bytes.chunks(16 * 1024) {
                    if send.send(chunk.to_vec()).await.is_err() {
                        return Ok(());
                    }
                }
            }
            Ok(None) => return Ok(()),
            Err(error) => return Err(message(error)),
        }
    }
}
