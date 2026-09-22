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

use super::provider_stream::request;
use futures_util::future::BoxFuture;
use maka_model::{ModelExecutor, ProviderKind};
use maka_plugins::{
    composition::Scope,
    contributions::{Catalog, Staged},
    fiber::Fiber,
    model::*,
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

struct Controlled {
    release: CancellationToken,
    retained: Arc<Mutex<Option<Context>>>,
}
impl ProviderAdapter for Controlled {
    fn open(
        &self,
        _: Lifetime,
        _: CancellationToken,
    ) -> BoxFuture<'_, Result<Arc<dyn Session>, Error>> {
        let session = Controlled {
            release: self.release.clone(),
            retained: self.retained.clone(),
        };
        Box::pin(async { Ok(Arc::new(session) as Arc<dyn Session>) })
    }
}
impl Session for Controlled {
    fn stream(&self, _: Request, context: Context) -> BoxFuture<'static, Result<(), Error>> {
        let release = self.release.clone();
        *self.retained.lock().unwrap() = Some(context.clone());
        Box::pin(async move {
            context
                .events
                .emit(ModelEvent::ResponseMetadata {
                    id: Some("accepted".into()),
                    model: None,
                    timestamp: None,
                })
                .await?;
            tokio::select! {
                _ = release.cancelled() => {}
                _ = context.cancellation.cancelled() => return Err(Error::Cancelled),
            }
            context
                .events
                .emit(ModelEvent::Finished {
                    reason: maka_runtime::model::ModelFinishReason::Stop,
                    usage: Default::default(),
                    provider_options: None,
                })
                .await
        })
    }
}
fn staged(release: CancellationToken, retained: Arc<Mutex<Option<Context>>>) -> Staged {
    let mut value = Staged::default();
    value
        .insert(
            "chat-completions",
            Adapter {
                provider: Arc::new(Controlled { release, retained }),
            },
        )
        .unwrap();
    value
}
#[tokio::test]
async fn publication_replacement_preserves_accepted_calls_rejects_stale_starts_and_bounds_idle_work()
 {
    let catalog = Catalog::default();
    let owner = Fiber::new("external.model", "external.model", Scope::Profile).unwrap();
    owner.begin_loading().unwrap();
    owner.ready().unwrap();
    owner.publish().unwrap();
    let release = CancellationToken::new();
    let retained = Arc::default();
    let registration = catalog
        .register(
            &owner.context(),
            staged(release.clone(), Arc::clone(&retained)),
        )
        .unwrap();
    let executor = ModelExecutor::new(2, Duration::from_millis(100))
        .unwrap()
        .with_catalog(catalog.clone());
    let input = || request(ProviderKind::OpenaiChat, "http://unused.example/v1".into());
    let original = executor.binding(&input().provider).unwrap();
    let source = original.source("chat-completions").unwrap();
    let mut accepted = executor
        .stream_with_adapter(input(), CancellationToken::new(), None, original.clone())
        .await
        .unwrap();
    assert!(matches!(
        accepted.next().await.unwrap().unwrap(),
        ModelEvent::ResponseMetadata { .. }
    ));
    drop(registration);
    let _replacement = catalog
        .register(
            &owner.context(),
            staged(CancellationToken::new(), Arc::default()),
        )
        .unwrap();
    let replacement = executor.binding(&input().provider).unwrap();
    assert_ne!(
        source.revision,
        replacement.source("chat-completions").unwrap().revision
    );
    assert!(
        executor
            .stream_with_adapter(input(), CancellationToken::new(), None, original)
            .await
            .is_err()
    );
    release.cancel();
    assert!(matches!(
        accepted.next().await.unwrap().unwrap(),
        ModelEvent::Finished { .. }
    ));
    assert!(accepted.next().await.is_none());
    accepted.cancel_and_wait().await;
    let context = retained.lock().unwrap().take().unwrap();
    assert!(matches!(
        context
            .transport
            .request(maka_plugins::http::Request {
                method: maka_plugins::http::Method::Get,
                url: "http://unused.example/".into(),
                headers: vec![],
                body: vec![],
            })
            .await,
        Err(Error::Cancelled)
    ));
    let mut stalled = executor
        .stream(input(), CancellationToken::new())
        .await
        .unwrap();
    assert!(matches!(
        stalled.next().await.unwrap().unwrap(),
        ModelEvent::ResponseMetadata { .. }
    ));
    assert!(matches!(
        stalled.next().await.unwrap(),
        Err(Error::TimedOut)
    ));
    stalled.cancel_and_wait().await;
    owner
        .shutdown(tokio::time::Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
}
