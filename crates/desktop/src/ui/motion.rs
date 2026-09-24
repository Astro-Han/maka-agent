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

//! One clock for every animation. A view that draws something moving asks for
//! the current time while it renders; the clock then redraws that view, and
//! only that view, about 30 times a second until it stops asking. Nothing
//! re-arms `request_animation_frame`, which would redraw at display rate.

use gpui_kit::{App, EntityId, Global, Window};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

const TICK: Duration = Duration::from_millis(33);
/// A view that has not drawn anything moving for this long is let go.
const LEASE: Duration = Duration::from_millis(300);

struct Clock {
    epoch: Instant,
    leases: HashMap<EntityId, Instant>,
    running: bool,
}

impl Global for Clock {}

impl Default for Clock {
    fn default() -> Self {
        Self {
            epoch: Instant::now(),
            leases: HashMap::new(),
            running: false,
        }
    }
}

/// Time since the clock started, and a redraw of the rendering view on the
/// next tick. Every animation reads this one epoch, so loaders stay in step.
/// Returns `None` when the system asks for reduced motion; callers then draw
/// their resting state.
pub fn now(window: &Window, cx: &mut App) -> Option<Duration> {
    if cx.reduce_motion() {
        return None;
    }
    let view = window.current_view();
    let clock = cx.default_global::<Clock>();
    let now = Instant::now();
    clock.leases.insert(view, now);
    let elapsed = now - clock.epoch;
    if !clock.running {
        clock.running = true;
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let idle = cx.update(|cx| {
                    let clock = cx.global_mut::<Clock>();
                    let now = Instant::now();
                    clock.leases.retain(|_, seen| now - *seen < LEASE);
                    let views: Vec<_> = clock.leases.keys().copied().collect();
                    clock.running = !views.is_empty();
                    for view in &views {
                        cx.notify(*view);
                    }
                    views.is_empty()
                });
                if idle {
                    return;
                }
            }
        })
        .detach();
    }
    Some(elapsed)
}

/// Position in a repeating cycle, from 0 up to 1.
pub fn cycle(elapsed: Duration, period: Duration) -> f32 {
    (elapsed.as_secs_f32() / period.as_secs_f32()).fract()
}
