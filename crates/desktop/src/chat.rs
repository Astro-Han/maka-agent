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

use crate::host::Host;
use gpui_kit::component::{
    ActiveTheme, Disableable, IconName, Sizable, StyledExt,
    bubble::{Bubble, BubbleContent, BubbleVariant},
    button::{Button, ButtonVariants},
    h_flex,
    input::{InputEvent, Textarea, TextareaState},
    message::{Message, MessageAlignment, MessageContent},
    message_scroller::{MessageScroller, MessageScrollerState},
    text::{TextView, TextViewState},
    v_flex,
};
use gpui_kit::{prelude::FluentBuilder, *};
use maka_client::{
    Client, RequestFailure,
    transcript::{LiveText, TranscriptBatch},
};
use maka_protocol::{
    interaction::{self, InteractionAnswer, InteractionRequest, InteractionSnapshot},
    message::{Placement, SubmitInput, SubmitResult},
    subscription::{
        AssistantObservationFrame, AssistantStreamKind, ObservationFrame,
        SessionAssistantStreamIdentity, SessionObservationSnapshot, SessionProjectionFrame,
        SubscriptionClosedReason, SubscriptionOpenInput, TranscriptAdvancedFrame, TranscriptPolicy,
    },
    transcript::{SessionTranscriptPageDirection, SessionTranscriptPageInput},
    turn::{MessageContent as TurnContent, TurnState},
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

const TAIL_BYTES: u64 = 16 * 1024;
const PAGE_BYTES: u64 = 64 * 1024;

#[derive(Clone, PartialEq)]
enum Kind {
    User,
    Assistant,
    Thinking,
    Tool,
    Failure,
}

#[derive(Clone, PartialEq)]
struct Item {
    key: SharedString,
    kind: Kind,
    text: SharedString,
}

enum Delivery {
    Sending,
    Failed(SharedString),
    Unknown(SharedString),
}

pub struct Chat {
    client: Client,
    session: String,
    subscription: Option<String>,
    error: Option<SharedString>,
    rows: BTreeMap<u64, Value>,
    through: Option<u64>,
    wanted: Option<u64>,
    paging: bool,
    live: Vec<(SessionAssistantStreamIdentity, LiveText)>,
    snapshot: Option<SessionObservationSnapshot>,
    items: Vec<Item>,
    markdown: HashMap<SharedString, (Entity<TextViewState>, SharedString)>,
    scroller: Entity<MessageScrollerState>,
    composer: Entity<TextareaState>,
    delivery: Option<Delivery>,
    answering: Option<String>,
    interaction_error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl Chat {
    pub fn new(
        client: Client,
        session: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 8)
                .placeholder("给 Maka 发消息，⌘↩ 发送")
        });
        let submit = cx.subscribe_in(&composer, window, |this, _, event, window, cx| {
            if let InputEvent::PressEnter {
                secondary: true, ..
            } = event
            {
                this.send(window, cx);
            }
        });
        let mut this = Self {
            client,
            session,
            subscription: None,
            error: None,
            rows: BTreeMap::new(),
            through: None,
            wanted: None,
            paging: false,
            live: Vec::new(),
            snapshot: None,
            items: Vec::new(),
            markdown: HashMap::new(),
            scroller,
            composer,
            delivery: None,
            answering: None,
            interaction_error: None,
            _subscriptions: vec![submit],
        };
        this.open(cx);
        this
    }

    pub fn session(&self) -> &str {
        &self.session
    }

    fn open(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let session = self.session.clone();
        let opening = cx.global::<Host>().spawn(async move {
            let opened = client
                .open_subscription(SubscriptionOpenInput {
                    session_id: session,
                    transcript: TranscriptPolicy::Tail {
                        max_bytes: TAIL_BYTES,
                    },
                })
                .await
                .map_err(|error| error.to_string())?;
            let durable = match &opened.transcript {
                Some(bootstrap) => bootstrap.durable.clone(),
                None => {
                    let _ = client.close_subscription(&opened.subscription_id).await;
                    return Err("Transcript bootstrap missing".to_string());
                }
            };
            match client
                .complete_transcript_page(&opened.subscription_id, durable)
                .await
            {
                Ok(batch) => Ok((opened, batch)),
                Err(error) => {
                    let _ = client.close_subscription(&opened.subscription_id).await;
                    Err(error.to_string())
                }
            }
        });
        cx.spawn(async move |this, cx| {
            let result = opening.await.and_then(|result| result);
            let _ = this.update(cx, |this, cx| match result {
                Ok((opened, batch)) => {
                    this.snapshot = Some(opened.snapshot);
                    this.install(batch, cx);
                    this.through = this.wanted;
                    // The Host starts pushing frames only after ready, so the
                    // snapshot above must already be installed.
                    this.subscription = Some(opened.subscription_id.clone());
                    this.ready(opened.subscription_id, cx);
                }
                Err(error) => {
                    this.error = Some(error.into());
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn ready(&mut self, subscription: String, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let ready = cx
            .global::<Host>()
            .spawn(async move { client.ready_subscription(&subscription).await });
        cx.spawn(async move |this, cx| {
            if let Err(error) = ready.await.and_then(|r| r.map_err(|e| e.to_string())) {
                let _ = this.update(cx, |this, cx| {
                    this.error = Some(error.into());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        if let Some(subscription) = self.subscription.take() {
            let client = self.client.clone();
            let closing = cx
                .global::<Host>()
                .spawn(async move { client.close_subscription(&subscription).await });
            cx.background_executor()
                .spawn(async move {
                    let _ = closing.await;
                })
                .detach();
        }
    }

    fn install(&mut self, batch: TranscriptBatch, cx: &mut Context<Self>) {
        for row in batch.rows {
            if row.value["type"] == "assistant" {
                let id = row.value["id"].as_str().unwrap_or_default();
                self.live.retain(|(stream, _)| stream.message_id != id);
            }
            self.rows.insert(row.sequence, row.value);
        }
        if let Some(through) = batch.through_sequence {
            self.wanted = self.wanted.max(Some(through));
        }
        self.rebuild(cx);
    }

    pub fn accept(&mut self, frames: Vec<ObservationFrame>, cx: &mut Context<Self>) {
        let mut changed = false;
        for frame in frames {
            if self.subscription.as_deref() != Some(frame.envelope().subscription_id) {
                continue;
            }
            match frame {
                ObservationFrame::Projection(frame) => {
                    let SessionProjectionFrame::SessionProjection { snapshot, .. } = *frame;
                    self.snapshot = Some(snapshot);
                    changed = true;
                }
                ObservationFrame::Transcript(TranscriptAdvancedFrame::TranscriptAdvanced {
                    through_sequence,
                    ..
                }) => {
                    self.wanted = self.wanted.max(Some(through_sequence));
                }
                ObservationFrame::Assistant(AssistantObservationFrame::SessionDelta {
                    delta,
                    ..
                }) => {
                    // A durable row may arrive before a trailing complete delta.
                    if self
                        .rows
                        .values()
                        .any(|row| row["id"] == delta.message_id && row["turnId"] == delta.turn_id)
                    {
                        continue;
                    }
                    let index = self.live.iter().position(|(id, _)| {
                        id.message_id == delta.message_id
                            && id.turn_id == delta.turn_id
                            && id.kind == delta.kind
                    });
                    let index = index.unwrap_or_else(|| {
                        self.live.push((
                            SessionAssistantStreamIdentity {
                                kind: delta.kind,
                                turn_id: delta.turn_id.clone(),
                                message_id: delta.message_id.clone(),
                            },
                            LiveText::default(),
                        ));
                        self.live.len() - 1
                    });
                    if let Err(error) = self.live[index].1.apply(&delta) {
                        self.error = Some(error.to_string().into());
                    }
                    changed = true;
                }
                ObservationFrame::Assistant(AssistantObservationFrame::Closed {
                    reason, ..
                }) => {
                    self.subscription = None;
                    self.error = Some(
                        match reason {
                            SubscriptionClosedReason::SessionRemoved => "会话已删除",
                            SubscriptionClosedReason::AccessRevoked => "访问权限已撤销",
                            SubscriptionClosedReason::SlowConsumer => "订阅因处理过慢被 Host 关闭",
                        }
                        .into(),
                    );
                    changed = true;
                }
                _ => {}
            }
        }
        self.fetch_newer(cx);
        if changed {
            self.rebuild(cx);
        }
    }

    fn fetch_newer(&mut self, cx: &mut Context<Self>) {
        let (Some(subscription), Some(wanted)) = (self.subscription.clone(), self.wanted) else {
            return;
        };
        if self.paging || self.through >= Some(wanted) {
            return;
        }
        self.paging = true;
        let client = self.client.clone();
        let anchor = self.through;
        let fetching = cx.global::<Host>().spawn(async move {
            let mut cursor = None;
            let mut batches = Vec::new();
            loop {
                let page = client
                    .transcript_page(SessionTranscriptPageInput {
                        subscription_id: subscription.clone(),
                        direction: SessionTranscriptPageDirection::Newer,
                        through_sequence: Some(wanted),
                        cursor: cursor.take(),
                        anchor_sequence: anchor,
                        max_bytes: PAGE_BYTES,
                    })
                    .await
                    .map_err(|error| error.to_string())?;
                let batch = client
                    .complete_transcript_page(&subscription, page)
                    .await
                    .map_err(|error| error.to_string())?;
                cursor = batch.next_cursor.clone();
                batches.push(batch);
                if cursor.is_none() {
                    return Ok::<_, String>(batches);
                }
            }
        });
        cx.spawn(async move |this, cx| {
            let result = fetching.await.and_then(|result| result);
            let _ = this.update(cx, |this, cx| {
                this.paging = false;
                match result {
                    Ok(batches) => {
                        for batch in batches {
                            this.install(batch, cx);
                        }
                        this.through = this.through.max(Some(wanted));
                        this.fetch_newer(cx);
                    }
                    Err(error) => {
                        this.error = Some(error.into());
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let mut items: Vec<Item> = self.rows.values().filter_map(project).collect();
        items.extend(
            self.live
                .iter()
                .filter(|(_, text)| !text.text.is_empty())
                .map(|(stream, text)| Item {
                    key: message_key(&stream.message_id, stream.kind).into(),
                    kind: match stream.kind {
                        AssistantStreamKind::Text => Kind::Assistant,
                        AssistantStreamKind::Thinking => Kind::Thinking,
                    },
                    text: text.text.clone().into(),
                }),
        );
        for item in items.iter().filter(|item| item.kind == Kind::Assistant) {
            match self.markdown.get_mut(&item.key) {
                Some((_, text)) if *text == item.text => {}
                Some((state, text)) => {
                    let extended = item.text.strip_prefix(text.as_str()).map(str::to_owned);
                    state.update(cx, |state, cx| match extended {
                        Some(suffix) => state.push_str(&suffix, cx),
                        None => state.set_text(&item.text, cx),
                    });
                    *text = item.text.clone();
                }
                None => {
                    let state = cx.new(|cx| TextViewState::markdown(&item.text, cx));
                    self.markdown
                        .insert(item.key.clone(), (state, item.text.clone()));
                }
            }
        }
        let old = std::mem::replace(&mut self.items, items);
        let same_prefix = old.len() <= self.items.len()
            && old.iter().zip(&self.items).all(|(a, b)| a.key == b.key);
        self.scroller.update(cx, |scroller, cx| {
            if !same_prefix {
                scroller.reset(self.items.len(), cx);
                return;
            }
            if let Some(changed) = old.iter().zip(&self.items).position(|(a, b)| a != b) {
                scroller.remeasure_items(changed..old.len(), cx);
            }
            if self.items.len() > old.len() {
                scroller.append(self.items.len() - old.len(), cx);
            }
        });
        cx.notify();
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(
            self.delivery,
            Some(Delivery::Sending | Delivery::Unknown(_))
        ) {
            return;
        }
        let text = self.composer.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        self.delivery = Some(Delivery::Sending);
        let client = self.client.clone();
        let input = SubmitInput {
            origin_host_epoch: client.identity.host_epoch.clone(),
            session_id: self.session.clone(),
            message_id: uuid::Uuid::new_v4().to_string(),
            content: TurnContent {
                text: text.clone(),
                display_text: None,
                attachments: None,
                directory_references: None,
                quotes: None,
                inline_references: None,
            },
            placement: Placement::NextTurn,
            input_selections: Default::default(),
            turn_orchestration: None,
        };
        let sending = cx
            .global::<Host>()
            .spawn(async move { client.submit_message(input).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = sending.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.delivery = match result {
                    Ok(Ok(SubmitResult::Blocked { message, .. })) => {
                        Some(Delivery::Failed(message.into()))
                    }
                    Ok(Ok(_)) => {
                        // Keep anything typed while the request was in flight.
                        if this.composer.read(cx).value() == text.as_str() {
                            this.composer
                                .update(cx, |composer, cx| composer.set_value("", window, cx));
                        }
                        None
                    }
                    Ok(Err(RequestFailure::NotDispatched(error))) => {
                        Some(Delivery::Failed(error.to_string().into()))
                    }
                    Ok(Err(error)) => Some(Delivery::Unknown(error.to_string().into())),
                    Err(error) => Some(Delivery::Unknown(error.into())),
                };
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn answer(
        &mut self,
        pending: InteractionSnapshot,
        answer: InteractionAnswer,
        cx: &mut Context<Self>,
    ) {
        if self.answering.is_some() {
            return;
        }
        self.answering = Some(pending.interaction_id().to_owned());
        self.interaction_error = None;
        let client = self.client.clone();
        let answering = cx
            .global::<Host>()
            .spawn(async move { client.answer_interaction(&pending, answer).await });
        cx.spawn(async move |this, cx| {
            let result = answering.await.and_then(|r| r.map_err(|e| e.to_string()));
            let _ = this.update(cx, |this, cx| {
                this.answering = None;
                if let Err(error) = result {
                    this.interaction_error = Some(error.into());
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn running(&self) -> bool {
        self.snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.root_turn.as_ref())
            .is_some_and(|turn| {
                matches!(
                    turn.state,
                    TurnState::Admitted(_) | TurnState::Created(_) | TurnState::Running(_)
                )
            })
    }

    fn render_item(&self, ix: usize, cx: &App) -> AnyElement {
        let Some(item) = self.items.get(ix) else {
            return div().into_any_element();
        };
        let body = match item.kind {
            Kind::User => Message::new()
                .alignment(MessageAlignment::End)
                .content(
                    MessageContent::new().bubble(
                        Bubble::new()
                            .with_variant(BubbleVariant::Secondary)
                            .content(BubbleContent::new().child(item.text.clone())),
                    ),
                )
                .into_any_element(),
            Kind::Assistant => match self.markdown.get(&item.key) {
                Some((state, _)) => TextView::new(state).selectable(true).into_any_element(),
                None => div().child(item.text.clone()).into_any_element(),
            },
            Kind::Thinking => div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(item.text.clone())
                .into_any_element(),
            Kind::Tool => div()
                .text_sm()
                .font_family(cx.theme().mono_font_family.clone())
                .text_color(cx.theme().muted_foreground)
                .child(item.text.clone())
                .into_any_element(),
            Kind::Failure => div()
                .text_sm()
                .text_color(cx.theme().danger)
                .child(item.text.clone())
                .into_any_element(),
        };
        div()
            .id(ElementId::Name(item.key.clone()))
            .px_6()
            .py_2()
            .child(body)
            .into_any_element()
    }

    fn render_interaction(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pending = self
            .snapshot
            .as_ref()?
            .interactions
            .pending()
            .first()?
            .clone();
        let busy = self.answering.as_deref() == Some(pending.interaction_id());
        let (title, detail, choices): (&str, String, Vec<(&str, &str, Value)>) = match pending
            .request()
        {
            InteractionRequest::Permissions {
                tool_use_id,
                request,
                ..
            } => {
                let allow = |scope: &str| json!({"decision": "allow", "permissions": request.permissions, "scope": scope});
                let mut choices = vec![("deny", "拒绝", json!({"decision": "deny"}))];
                if tool_use_id.is_some() {
                    choices.push(("once", "允许一次", allow("once")));
                }
                choices.push(("session", "本会话允许", allow("session")));
                let detail = match &request.command {
                    Some(command) => format!(
                        "{}\n\n$ {}\n  (cwd: {})",
                        request.reason, command.command, command.cwd
                    ),
                    None => request.reason.clone(),
                };
                (
                    "需要额外权限",
                    detail,
                    choices
                        .into_iter()
                        .map(|(id, label, decision)| {
                            (
                                id,
                                label,
                                json!({"kind": "permissions", "decision": decision}),
                            )
                        })
                        .collect(),
                )
            }
            InteractionRequest::ClientCapability { target, .. } => (
                "客户端能力请求",
                serde_json::to_string_pretty(target).unwrap_or_default(),
                vec![
                    (
                        "deny",
                        "拒绝",
                        json!({"kind": "client_capability", "decision": "deny"}),
                    ),
                    (
                        "allow",
                        "允许",
                        json!({"kind": "client_capability", "decision": "allow"}),
                    ),
                ],
            ),
            _ => ("有待回答的问题", "请在其他客户端中回答。".into(), vec![]),
        };
        let buttons = choices
            .into_iter()
            .enumerate()
            .map(|(ix, (id, label, value))| {
                let pending = pending.clone();
                let button =
                    Button::new(SharedString::from(format!("interaction-{id}")))
                        .label(label)
                        .small()
                        .disabled(busy)
                        .on_click(cx.listener(
                            move |this, _, _, cx| match interaction::decode_answer(&value) {
                                Ok(answer) => this.answer(pending.clone(), answer, cx),
                                Err(error) => {
                                    this.interaction_error = Some(error.to_string().into());
                                    cx.notify();
                                }
                            },
                        ));
                // Choices run from deny to the broadest grant; the default is the narrowest allow.
                if ix == 1 { button.primary() } else { button }
            });
        Some(
            v_flex()
                .mx_6()
                .mb_3()
                .p_4()
                .gap_3()
                .rounded(cx.theme().radius_lg)
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().secondary)
                .child(div().font_semibold().child(title))
                .child(
                    div()
                        .text_sm()
                        .font_family(cx.theme().mono_font_family.clone())
                        .child(detail),
                )
                .when_some(self.interaction_error.clone(), |this, error| {
                    this.child(div().text_sm().text_color(cx.theme().danger).child(error))
                })
                .child(h_flex().justify_end().gap_2().children(buttons))
                .into_any_element(),
        )
    }

    fn render_composer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let sending = matches!(self.delivery, Some(Delivery::Sending));
        let status: Option<(SharedString, bool)> = match &self.delivery {
            Some(Delivery::Failed(error)) => Some((format!("未发送：{error}").into(), true)),
            Some(Delivery::Unknown(error)) => {
                Some((format!("发送结果未知，未自动重试：{error}").into(), true))
            }
            _ if self.running() => Some(("正在回复…".into(), false)),
            _ => None,
        };
        v_flex()
            .px_6()
            .pb_4()
            .gap_2()
            .when_some(status, |this, (text, error)| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(if error {
                            cx.theme().danger
                        } else {
                            cx.theme().muted_foreground
                        })
                        .child(text),
                )
            })
            .child(
                h_flex()
                    .items_end()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Textarea::new(&self.composer)),
                    )
                    .child(
                        Button::new("send")
                            .primary()
                            .icon(IconName::ArrowUp)
                            .tooltip("发送 ⌘↩")
                            .loading(sending)
                            .disabled(
                                self.subscription.is_none()
                                    || matches!(self.delivery, Some(Delivery::Unknown(_))),
                            )
                            .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
                    ),
            )
    }
}

impl Render for Chat {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chat = cx.weak_entity();
        v_flex()
            .size_full()
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .px_6()
                        .py_2()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            })
            .child(
                div().flex_1().min_h_0().child(
                    MessageScroller::new("transcript", self.scroller.clone(), move |ix, _, cx| {
                        chat.upgrade()
                            .map(|chat| chat.read(cx).render_item(ix, cx))
                            .unwrap_or_else(|| div().into_any_element())
                    })
                    .with_jump_button_label("回到最新")
                    .size_full(),
                ),
            )
            .children(self.render_interaction(cx))
            .child(self.render_composer(cx))
    }
}

fn message_key(message_id: &str, kind: AssistantStreamKind) -> String {
    match kind {
        AssistantStreamKind::Text => format!("assistant:{message_id}"),
        AssistantStreamKind::Thinking => format!("thinking:{message_id}"),
    }
}

fn project(row: &Value) -> Option<Item> {
    let id = row["id"].as_str().unwrap_or_default();
    let text = |value: &Value| {
        value
            .as_str()
            .filter(|text| !text.trim().is_empty())
            .map(str::to_owned)
    };
    let (key, kind, text) = match row["type"].as_str()? {
        "user" => (
            format!("user:{id}"),
            Kind::User,
            text(&row["displayText"]).or_else(|| text(&row["text"]))?,
        ),
        "assistant" => match text(&row["text"]) {
            Some(body) => (
                message_key(id, AssistantStreamKind::Text),
                Kind::Assistant,
                body,
            ),
            None => (
                message_key(id, AssistantStreamKind::Thinking),
                Kind::Thinking,
                text(&row["thinking"]["text"])?,
            ),
        },
        "tool_call" => (
            format!("tool:{id}"),
            Kind::Tool,
            format!(
                "▸ {} {}",
                row["toolName"].as_str().unwrap_or("tool"),
                row["args"]["command"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| row["args"].to_string())
            ),
        ),
        "turn_state" if row["status"] == "failed" => (
            format!("failure:{id}"),
            Kind::Failure,
            format!(
                "运行失败：{}",
                row["failureMessage"].as_str().unwrap_or("未知错误")
            ),
        ),
        _ => return None,
    };
    Some(Item {
        key: key.into(),
        kind,
        text: text.into(),
    })
}
