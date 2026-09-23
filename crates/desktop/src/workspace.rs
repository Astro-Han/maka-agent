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

use crate::{
    chat::Chat,
    host::{self, Host},
};
use gpui_kit::component::{
    ActiveTheme, Disableable, IconName, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    h_flex,
    sidebar::{Sidebar, SidebarGroup, SidebarHeader, SidebarMenu, SidebarMenuItem},
    v_flex,
};
use gpui_kit::{prelude::FluentBuilder, *};
use maka_client::{Client, Notification};
use maka_protocol::session::{
    SessionCatalogProjection, SessionCatalogQueryInput, SessionCatalogQueryResult,
};
use serde_json::json;
use std::{path::PathBuf, time::Duration};
use tokio::sync::mpsc;

/// Deltas arrive at the provider's chunk rate; one repaint per window is enough.
const STREAM_FRAME: Duration = Duration::from_millis(100);

enum Connection {
    Connecting,
    Ready(Client),
    Failed(SharedString),
}

pub struct Workspace {
    root: PathBuf,
    connection: Connection,
    sessions: Vec<SessionCatalogProjection>,
    sessions_error: Option<SharedString>,
    chat: Option<Entity<Chat>>,
    _tasks: Vec<Task<()>>,
}

impl Workspace {
    pub fn new(root: PathBuf, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            root,
            connection: Connection::Connecting,
            sessions: Vec::new(),
            sessions_error: None,
            chat: None,
            _tasks: Vec::new(),
        };
        this.connect(cx);
        this
    }

    fn connect(&mut self, cx: &mut Context<Self>) {
        self.connection = Connection::Connecting;
        let connecting = cx.global::<Host>().spawn(host::connect(self.root.clone()));
        self._tasks = vec![cx.spawn(async move |this, cx| {
            let result = connecting.await.and_then(|result| result);
            let Ok(notifications) = this.update(cx, |this, cx| match result {
                Ok((client, notifications)) => {
                    this.connection = Connection::Ready(client);
                    this.load_sessions(cx);
                    cx.notify();
                    Some(notifications)
                }
                Err(error) => {
                    this.connection = Connection::Failed(error.into());
                    cx.notify();
                    None
                }
            }) else {
                return;
            };
            if let Some(notifications) = notifications {
                Self::pump(this, notifications, cx).await;
            }
        })];
    }

    async fn pump(
        this: WeakEntity<Self>,
        mut notifications: mpsc::Receiver<Notification>,
        cx: &mut AsyncApp,
    ) {
        while let Some(first) = notifications.recv().await {
            let mut batch = vec![first];
            while let Ok(next) = notifications.try_recv() {
                batch.push(next);
            }
            if this.update(cx, |this, cx| this.apply(batch, cx)).is_err() {
                return;
            }
            cx.background_executor().timer(STREAM_FRAME).await;
        }
        let _ = this.update(cx, |this, cx| {
            this.connection = Connection::Failed("Host connection closed".into());
            cx.notify();
        });
    }

    fn apply(&mut self, batch: Vec<Notification>, cx: &mut Context<Self>) {
        let mut catalog_changed = false;
        let mut frames = Vec::new();
        for notification in batch {
            match notification {
                Notification::Catalog(change) => {
                    catalog_changed |= change.kind == "session.catalog.changed";
                }
                Notification::Observation(frame) => frames.push(*frame),
            }
        }
        if let Some(chat) = &self.chat
            && !frames.is_empty()
        {
            chat.update(cx, |chat, cx| chat.accept(frames, cx));
        }
        if catalog_changed {
            self.load_sessions(cx);
        }
    }

    fn client(&self) -> Option<Client> {
        match &self.connection {
            Connection::Ready(client) => Some(client.clone()),
            _ => None,
        }
    }

    fn load_sessions(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client() else {
            return;
        };
        let loading = cx.global::<Host>().spawn(async move {
            client
                .session_catalog(SessionCatalogQueryInput::ListStart)
                .await
        });
        cx.spawn(async move |this, cx| {
            let result = loading.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(Ok(SessionCatalogQueryResult::Page { sessions, .. })) => {
                        this.sessions = sessions
                            .into_iter()
                            .filter(|session| !session.is_archived)
                            .collect();
                        this.sessions_error = None;
                    }
                    Ok(Ok(_)) => {}
                    Ok(Err(error)) => this.sessions_error = Some(error.to_string().into()),
                    Err(error) => this.sessions_error = Some(error.into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn create_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.client() else {
            return;
        };
        let workspace = std::env::current_dir().unwrap_or_else(|_| self.root.clone());
        let creating = cx.global::<Host>().spawn(async move {
            let input = maka_protocol::session::decode_session_create_input(&json!({
                "sessionId": uuid::Uuid::new_v4().to_string(),
                "name": "新会话",
                "workspace": {"kind": "host_path", "path": workspace},
                "modelTarget": {"kind": "default"}
            }))
            .map_err(|error| error.to_string())?;
            client
                .create_session(input)
                .await
                .map_err(|error| error.to_string())
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = creating.await.and_then(|result| result);
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(session) => {
                    let id = session.id.clone();
                    this.sessions.insert(0, session);
                    this.open(id, window, cx);
                }
                Err(error) => {
                    this.sessions_error = Some(error.into());
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn open(&mut self, session: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.client() else {
            return;
        };
        if self
            .chat
            .as_ref()
            .is_some_and(|chat| chat.read(cx).session() == session)
        {
            return;
        }
        if let Some(chat) = self.chat.take() {
            chat.update(cx, |chat, cx| chat.close(cx));
        }
        self.chat = Some(cx.new(|cx| Chat::new(client, session, window, cx)));
        cx.notify();
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self
            .chat
            .as_ref()
            .map(|chat| chat.read(cx).session().to_owned());
        let items = self.sessions.iter().map(|session| {
            let id = session.id.clone();
            let label = if session.name.is_empty() {
                "未命名会话".to_string()
            } else {
                session.name.clone()
            };
            SidebarMenuItem::new(label)
                .active(selected.as_deref() == Some(id.as_str()))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open(id.clone(), window, cx);
                }))
        });
        Sidebar::new("sessions")
            .w(px(260.))
            .header(
                SidebarHeader::new().child(
                    h_flex()
                        .w_full()
                        .justify_between()
                        .child(div().font_semibold().child("Maka"))
                        .child(
                            Button::new("new-session")
                                .ghost()
                                .small()
                                .icon(IconName::Plus)
                                .tooltip("新建会话")
                                .disabled(self.client().is_none())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.create_session(window, cx);
                                })),
                        ),
                ),
            )
            .child(SidebarGroup::new("会话").child(SidebarMenu::new().children(items)))
    }

    fn render_main(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(chat) = &self.chat {
            return chat.clone().into_any_element();
        }
        let message: SharedString = match &self.connection {
            Connection::Connecting => "正在连接 Host…".into(),
            Connection::Failed(error) => format!("无法连接 Host：{error}").into(),
            Connection::Ready(_) => match &self.sessions_error {
                Some(error) => format!("无法读取会话：{error}").into(),
                None if self.sessions.is_empty() => "还没有会话".into(),
                None => "选择一个会话".into(),
            },
        };
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .text_color(cx.theme().muted_foreground)
            .child(message)
            .when(matches!(self.connection, Connection::Failed(_)), |this| {
                this.child(
                    Button::new("reconnect")
                        .label("重新连接")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.connect(cx);
                            cx.notify();
                        })),
                )
            })
            .into_any_element()
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .size_full()
            .items_stretch()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_sidebar(cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(self.render_main(cx)),
            )
    }
}
