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

//! Behavior the client relies on from gpui-kit, checked against the version
//! we depend on. An ignored test names a gap the client has to cover.

use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, TestAppContext,
    Window,
    component::{Root, button::Button, popover::Popover, text::TextViewState},
    div, px, size,
};
use std::{cell::RefCell, rc::Rc};

fn rendered(state: &Entity<TextViewState>, cx: &mut TestAppContext) -> String {
    state.update(cx, |state, cx| state.select_all(cx));
    state.read_with(cx, |state, _| state.selected_text())
}

/// Streams `source` one character at a time, letting each append parse on its
/// own, and returns the rendered text.
fn streamed(source: &str, cx: &mut TestAppContext) -> String {
    let state = cx.new(|cx| TextViewState::markdown("", cx));
    for ch in source.chars() {
        state.update(cx, |state, cx| state.push_str(&ch.to_string(), cx));
        cx.run_until_parked();
    }
    rendered(&state, cx)
}

fn parsed(source: &str, cx: &mut TestAppContext) -> String {
    let state = cx.new(|cx| TextViewState::markdown(source, cx));
    cx.run_until_parked();
    rendered(&state, cx)
}

#[gpui_kit::test]
fn a_streamed_table_renders_like_a_full_parse(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let source = "Intro\n\n| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n\nAfter the table.\n";
    let full = parsed(source, cx);
    assert!(full.contains('3') && !full.contains('|'), "{full:?}");
    assert_eq!(streamed(source, cx), full);
}

#[gpui_kit::test]
fn a_streamed_list_and_fence_render_like_a_full_parse(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let source = "Steps:\n\n1. one\n2. two\n\n```rust\nfn main() {}\n```\n\nDone **now**.\n";
    assert_eq!(streamed(source, cx), parsed(source, cx));
}

#[gpui_kit::test]
#[ignore = "gpui-kit 0.6.6 shows unclosed markers literally while streaming"]
fn an_unclosed_emphasis_does_not_show_its_marker_while_streaming(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let state = cx.new(|cx| TextViewState::markdown("", cx));
    state.update(cx, |state, cx| state.push_str("Hello **bol", cx));
    cx.run_until_parked();
    assert_eq!(rendered(&state, cx), "Hello bol\n");
}

struct PopoverHost {
    changes: Rc<RefCell<Vec<bool>>>,
}

impl Render for PopoverHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let changes = self.changes.clone();
        div().size_full().child(
            Popover::new("popover")
                .trigger(Button::new("trigger").label("Open"))
                .content(|_, _, _| div().size(px(80.)).child("Inside"))
                .on_open_change(move |open, _, _| changes.borrow_mut().push(*open)),
        )
    }
}

fn open_popover(cx: &mut TestAppContext) -> (gpui_kit::AnyWindowHandle, Rc<RefCell<Vec<bool>>>) {
    cx.update(gpui_kit::init);
    let changes = Rc::new(RefCell::new(Vec::new()));
    let handle = cx.open_window(size(px(400.), px(300.)), {
        let changes = changes.clone();
        move |window, cx| {
            let view = cx.new(|_| PopoverHost { changes });
            Root::new(view, window, cx)
        }
    });
    let handle = handle.into();
    step(handle, cx, |window, cx| window.click("trigger", cx));
    assert_eq!(&*changes.borrow(), &[true]);
    (handle, changes)
}

fn step(
    handle: gpui_kit::AnyWindowHandle,
    cx: &mut TestAppContext,
    action: impl FnOnce(&mut Window, &mut gpui_kit::App),
) {
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        action(window, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn clicking_the_trigger_of_an_open_popover_closes_it(cx: &mut TestAppContext) {
    let (handle, changes) = open_popover(cx);
    step(handle, cx, |window, cx| window.click("trigger", cx));
    assert_eq!(&*changes.borrow(), &[true, false]);
}

#[gpui_kit::test]
fn an_opened_popover_takes_focus_so_escape_closes_it(cx: &mut TestAppContext) {
    let (handle, changes) = open_popover(cx);
    step(handle, cx, |window, cx| window.press("escape", cx));
    assert_eq!(&*changes.borrow(), &[true, false]);
}
