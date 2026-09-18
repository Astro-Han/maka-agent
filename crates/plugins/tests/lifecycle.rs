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

use std::sync::{Arc, Mutex};
use std::time::Duration;

use maka_plugins::{
    Error,
    composition::Scope,
    fiber::{Effect, Fiber, Phase},
};
use tokio::{sync::Notify, time::Instant};

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(2)
}

#[tokio::test]
async fn retirement_closes_admission_signals_before_drain_and_keeps_cleanup_owned() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let fiber = Fiber::new("graph", "root", Scope::Profile).unwrap();
    fiber.begin_loading().unwrap();
    let context = fiber.context();
    let child = Fiber::new("graph", "child", Scope::Profile).unwrap();
    child.begin_loading().unwrap();
    let child_events = events.clone();
    assert!(
        child
            .context()
            .own(Effect::new(
                "child",
                || {},
                move || async move {
                    child_events.lock().unwrap().push("child");
                    Ok(())
                }
            ))
            .is_ok()
    );
    assert!(fiber.own_child(child).is_ok());
    let process_stop = Arc::new(Notify::new());
    let cleanup_entered = Arc::new(Notify::new());
    let cleanup_release = Arc::new(Notify::new());
    let stop = process_stop.clone();
    let closed = events.clone();
    assert!(
        context
            .own(Effect::new(
                "process",
                move || {
                    stop.notify_one();
                },
                move || async move {
                    closed.lock().unwrap().push("process");
                    Ok(())
                }
            ))
            .is_ok()
    );
    let stop = process_stop.clone();
    let entered = cleanup_entered.clone();
    let release = cleanup_release.clone();
    let closed = events.clone();
    assert!(
        context
            .own(Effect::new(
                "consumer",
                || {},
                move || async move {
                    stop.notified().await;
                    entered.notify_one();
                    release.notified().await;
                    closed.lock().unwrap().push("consumer");
                    Ok(())
                }
            ))
            .is_ok()
    );
    fiber.ready().unwrap();
    assert!(context.is_ready());
    assert!(matches!(context.admit(), Err(Error::Retired)));
    fiber.publish().unwrap();
    let call = context.admit().unwrap();
    assert_eq!(
        fiber.shutdown(Instant::now()).await,
        Err(Error::CleanupPending)
    );
    assert!(context.stopping().unwrap().is_cancelled());
    assert!(matches!(context.admit(), Err(Error::Retired)));
    assert_eq!(fiber.phase(), Phase::Unloading);
    drop(call);
    tokio::time::timeout_at(deadline(), cleanup_entered.notified())
        .await
        .unwrap();
    assert_eq!(*events.lock().unwrap(), ["child"]);
    cleanup_release.notify_one();
    fiber.shutdown(deadline()).await.unwrap();
    assert_eq!(*events.lock().unwrap(), ["child", "consumer", "process"]);
    assert_eq!(fiber.phase(), Phase::Disposed);
    assert!(fiber.publish().is_err());
    let activation = context.identity().unwrap().activation;
    drop(fiber);
    assert!(matches!(context.admit(), Err(Error::Retired)));
    let replacement = Fiber::new("graph", "root", Scope::Profile).unwrap();
    assert_ne!(replacement.identity().activation, activation);
    replacement.shutdown(deadline()).await.unwrap();
}

#[tokio::test]
async fn cleanup_failure_is_reported_without_skipping_other_resources_or_reviving() {
    let cleaned = Arc::new(Notify::new());
    let fiber = Fiber::new("graph", "root", Scope::Profile).unwrap();
    fiber.begin_loading().unwrap();
    let done = cleaned.clone();
    assert!(
        fiber
            .context()
            .own(Effect::new(
                "first",
                || {},
                move || async move {
                    done.notify_one();
                    Ok(())
                }
            ))
            .is_ok()
    );
    assert!(
        fiber
            .context()
            .own(Effect::new(
                "failure",
                || {},
                || async { Err("cleanup not confirmed".into()) }
            ))
            .is_ok()
    );
    let result = fiber.shutdown(deadline()).await;
    assert!(
        matches!(result, Err(Error::Cleanup(ref errors)) if errors == &["failure: cleanup not confirmed"])
    );
    tokio::time::timeout_at(deadline(), cleaned.notified())
        .await
        .unwrap();
    assert_eq!(fiber.phase(), Phase::Failed);
    assert!(fiber.begin_loading().is_err());
    assert!(fiber.ready().is_err());
    assert!(matches!(fiber.context().admit(), Err(Error::Retired)));
}
