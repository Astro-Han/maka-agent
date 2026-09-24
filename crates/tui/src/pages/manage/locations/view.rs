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

use super::{Command, Manage};
use crate::{
    app::{Action, App},
    pages::manage::Dialog,
    ui::{Node, On, Role, Sheet, Size, Tone},
    view::safe,
};

/// Where a project lives on the Host: a read-only viewer that takes focus
/// and scrolls, with paging and retry at the bottom left.
pub(in crate::pages::manage) fn sheet(app: &App, dialog: &Dialog) -> Sheet<Action> {
    let locations = dialog.locations.as_ref().expect("locations reader");
    let mut sheet = Sheet::new(
        format!("locations:{}", dialog.target.name),
        app.i18n.text("project-locations"),
    )
    .text(
        "name",
        &safe(locations.name.as_deref().unwrap_or(&dialog.target.name)),
        Tone::Normal,
    );
    let error = dialog.error.or(locations.error);
    if let Some(error) = error {
        sheet = sheet.text("error", &app.i18n.text(error), Tone::Warning);
    } else if !locations.ready() {
        sheet = sheet.text("loading", &app.i18n.text("projects-loading"), Tone::Subtle);
    } else if locations.rows.is_empty() {
        sheet = sheet.text(
            "empty",
            &app.i18n.text("project-locations-empty"),
            Tone::Subtle,
        );
    } else {
        let rows = locations
            .rows
            .iter()
            .map(|(index, location)| {
                let tags = [
                    (
                        Some(*index) == locations.preferred,
                        "project-location-preferred",
                    ),
                    (location.is_worktree, "project-location-worktree"),
                ]
                .into_iter()
                .filter(|(yes, _)| *yes)
                .map(|(_, key)| app.i18n.text(key))
                .collect::<Vec<_>>();
                let mut lines = vec![];
                if !tags.is_empty() {
                    lines.push(Node::text("tags", vec![(tags.join(" · "), Tone::Subtle)]));
                }
                lines.push(Node::text(
                    "path",
                    vec![(safe(&location.path), Tone::Normal)],
                ));
                Node::column(index.to_string(), lines)
            })
            .collect();
        let height = app.frame_size.map_or(24, |(_, height)| height);
        sheet = sheet.body(
            Node::scroll("paths", Node::column("rows", rows).gap(1))
                .on(On::Scroll)
                .size(Size::Upto(height.saturating_sub(14).max(3))),
        );
    }
    let command = |command: Command| {
        let action = Action::Manage(Manage::Locations(command));
        let enabled = app.enabled(&action);
        (action, enabled)
    };
    let (previous, can_previous) = command(Command::Previous);
    let (next, can_next) = command(Command::Next);
    if can_previous || can_next {
        sheet = sheet
            .aside(
                "previous",
                format!(
                    "{} {}",
                    app.chrome.symbol("‹", "<"),
                    app.i18n.text("sessions-previous")
                ),
                previous,
                can_previous,
            )
            .aside(
                "next",
                format!(
                    "{} {}",
                    app.i18n.text("sessions-next"),
                    app.chrome.symbol("›", ">")
                ),
                next,
                can_next,
            );
    }
    if error.is_some() {
        let (refresh, can_refresh) = command(Command::Refresh);
        sheet = sheet.aside("refresh", app.i18n.text("list-retry"), refresh, can_refresh);
    }
    sheet.button(
        "close",
        app.i18n.text("project-locations-close"),
        Role::Normal,
        Action::Manage(Manage::Close),
        true,
    )
}
