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

use futures_util::future::BoxFuture;
use maka_plugins::{
    composition::Scope,
    fiber::Fiber,
    services::{Services, method},
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Echo {
    started: Notify,
    cancelled: Notify,
    release: Notify,
}
impl method::Method<Vec<u8>, Vec<u8>> for Echo {
    fn call(
        &self,
        input: Vec<u8>,
        context: method::Context,
    ) -> BoxFuture<'_, Result<Vec<u8>, method::Error>> {
        Box::pin(async move {
            if input.is_empty() {
                self.started.notify_one();
                context.cancellation.cancelled().await;
                self.cancelled.notify_one();
                self.release.notified().await;
            }
            Ok(input)
        })
    }
}

#[tokio::test]
async fn callable_services_preserve_native_values_and_own_abandoned_calls_until_cleanup() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let provider = Fiber::new("example", "provider", Scope::Profile).unwrap();
        provider.begin_loading().unwrap();
        provider.ready().unwrap();
        provider.publish().unwrap();
        let services = Services::default().view();
        let echo = Arc::new(Echo::default());
        let registration = services
            .register_method(&provider.context(), "echo", echo.clone())
            .unwrap();
        let handle = services.method("echo").unwrap().unwrap();

        let input = vec![1_u8, 2, 3];
        let allocation = input.as_ptr();
        let output: Vec<u8> = handle
            .call(input, None, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(
            output.as_ptr(),
            allocation,
            "native peers must not serialize their values"
        );
        let output: Value = handle
            .call(json!([4, 5, 6]), None, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(output, json!([4, 5, 6]));
        assert!(matches!(
            handle
                .call::<_, Value>(json!({"wrong":"shape"}), None, CancellationToken::new())
                .await,
            Err(method::Error::Invalid(_))
        ));

        let old = handle.clone();
        let caller = tokio::spawn(async move {
            old.call::<_, Vec<u8>>(Vec::<u8>::new(), None, CancellationToken::new())
                .await
        });
        echo.started.notified().await;
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        echo.cancelled.notified().await;
        drop(registration);
        assert!(matches!(
            handle
                .call::<_, Value>(json!([7]), None, CancellationToken::new())
                .await,
            Err(method::Error::Retired)
        ));
        assert_eq!(
            provider.context().active_calls(),
            1,
            "abandoning a reply must not release the active provider"
        );
        echo.release.notify_one();
        provider
            .shutdown(tokio::time::Instant::now() + Duration::from_secs(1))
            .await
            .unwrap();
    })
    .await
    .unwrap();
}
