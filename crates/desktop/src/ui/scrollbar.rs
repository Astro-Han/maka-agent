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

//! A macOS-style overlay scrollbar for a `list()`: hidden at rest, shown
//! while the content moves, then faded out. A streaming transcript moves on
//! every update, so the bar holds at full opacity for the whole reply; the
//! hold costs one timer, and only the fade draws frames.

use super::motion;
use crate::theme::theme;
use gpui_kit::{
    App, Bounds, DispatchPhase, EntityId, IntoElement, ListState, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Styled, Task, Window, canvas, fill, point, px, size,
};
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

const HOLD: Duration = Duration::from_millis(900);
const FADE: Duration = Duration::from_millis(350);
const TRACK: f32 = 11.;
const MIN_THUMB: f32 = 28.;

#[derive(Default)]
struct State {
    offset: Option<Pixels>,
    moved: Option<Instant>,
    hovered: bool,
    /// Where in the thumb the pointer grabbed it.
    grab: Option<Pixels>,
    wake: Option<Task<()>>,
}

#[derive(Clone, Default)]
pub struct Scrollbar(Rc<RefCell<State>>);

impl Scrollbar {
    /// The bar, absolutely placed along the right edge of its parent.
    /// `view` is the view to redraw while it changes.
    pub fn render(
        &self,
        list: &ListState,
        view: EntityId,
        window: &mut Window,
        cx: &mut App,
    ) -> impl IntoElement + use<> {
        let viewport = list.viewport_bounds();
        let max = list.max_offset_for_scrollbar().y;
        let offset = -list.scroll_px_offset_for_scrollbar().y;
        let opacity = self.observe(offset, max, view, window, cx);
        let theme = theme(cx);
        let state = self.0.clone();
        let list = list.clone();
        let (rest, engaged) = (theme.muted.opacity(0.45), theme.muted);
        canvas(
            |_, _, _| {},
            move |track: Bounds<Pixels>, _, window, _| {
                if max <= px(0.5) || viewport.size.height <= px(0.) {
                    let mut state = state.borrow_mut();
                    state.hovered = false;
                    state.grab = None;
                    return;
                }
                let height = track.size.height;
                let content = height + max;
                let thumb_height = (height * (height / content)).max(px(MIN_THUMB)).min(height);
                let travel = height - thumb_height;
                let progress = (offset / max).clamp(0., 1.);
                let thumb_top = track.top() + travel * progress;
                let (hovered, grabbed) = {
                    let state = state.borrow();
                    (state.hovered, state.grab.is_some())
                };
                let width = if hovered || grabbed { 8. } else { 5. };
                let color = if hovered || grabbed { engaged } else { rest };
                if opacity > 0. {
                    window.paint_quad(
                        fill(
                            Bounds::new(
                                point(track.right() - px(2. + width), thumb_top),
                                size(px(width), thumb_height),
                            ),
                            color.opacity(color.a * opacity),
                        )
                        .corner_radii(px(width / 2.)),
                    );
                }
                let thumb = Bounds::new(
                    point(track.left(), thumb_top),
                    size(track.size.width, thumb_height),
                );
                let set_offset = {
                    let list = list.clone();
                    move |y: Pixels, grab: Pixels| {
                        let ratio = if travel > px(0.) {
                            ((y - grab - track.top()) / travel).clamp(0., 1.)
                        } else {
                            0.
                        };
                        list.set_offset_from_scrollbar(point(px(0.), -(max * ratio)));
                    }
                };
                window.on_mouse_event({
                    let state = state.clone();
                    let list = list.clone();
                    let set_offset = set_offset.clone();
                    move |event: &MouseDownEvent, phase, window, cx| {
                        if phase != DispatchPhase::Bubble
                            || event.button != MouseButton::Left
                            || !track.contains(&event.position)
                        {
                            return;
                        }
                        let grab = if thumb.contains(&event.position) {
                            event.position.y - thumb.top()
                        } else {
                            thumb_height / 2.
                        };
                        state.borrow_mut().grab = Some(grab);
                        list.scrollbar_drag_started();
                        set_offset(event.position.y, grab);
                        cx.stop_propagation();
                        window.refresh();
                    }
                });
                window.on_mouse_event({
                    let state = state.clone();
                    move |event: &MouseMoveEvent, phase, window, _| {
                        if phase != DispatchPhase::Bubble {
                            return;
                        }
                        let grab = state.borrow().grab;
                        if let Some(grab) = grab {
                            set_offset(event.position.y, grab);
                            window.refresh();
                            return;
                        }
                        let hovered = track.contains(&event.position);
                        if state.borrow().hovered != hovered {
                            state.borrow_mut().hovered = hovered;
                            window.refresh();
                        }
                    }
                });
                window.on_mouse_event({
                    let state = state.clone();
                    move |_: &MouseUpEvent, phase, window, _| {
                        if phase == DispatchPhase::Bubble
                            && state.borrow_mut().grab.take().is_some()
                        {
                            list.scrollbar_drag_ended();
                            window.refresh();
                        }
                    }
                });
            },
        )
        .absolute()
        .top_0()
        .right_0()
        .h_full()
        .w(px(TRACK))
    }

    /// Records the offset and returns the bar's opacity now, arranging the
    /// next redraw it needs.
    fn observe(
        &self,
        offset: Pixels,
        max: Pixels,
        view: EntityId,
        window: &Window,
        cx: &mut App,
    ) -> f32 {
        let mut state = self.0.borrow_mut();
        let now = Instant::now();
        match state.offset {
            // The first sight of the list is where it opened, not a scroll.
            None => state.offset = Some(offset),
            Some(last) if (last - offset).abs() > px(0.5) => {
                state.offset = Some(offset);
                state.moved = Some(now);
            }
            _ => {}
        }
        if max <= px(0.5) {
            return 0.;
        }
        if state.hovered || state.grab.is_some() {
            return 1.;
        }
        let Some(moved) = state.moved else {
            return 0.;
        };
        let since = now - moved;
        if since < HOLD {
            let wait = HOLD - since + Duration::from_millis(16);
            state.wake = Some(cx.spawn(async move |cx| {
                cx.background_executor().timer(wait).await;
                cx.update(|cx| cx.notify(view));
            }));
            return 1.;
        }
        state.wake = None;
        if since < HOLD + FADE {
            drop(state);
            motion::now(window, cx);
            return 1. - (since - HOLD).as_secs_f32() / FADE.as_secs_f32();
        }
        0.
    }
}
