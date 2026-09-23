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

use super::*;
use maka_protocol::{
    project::{Query, QueryResult},
    turn::DirectoryReference,
};

impl App {
    pub(crate) fn directory_reference_active(&self) -> bool {
        self.management
            .dialog
            .as_ref()
            .is_some_and(|d| d.kind == Kind::Reference)
    }
    pub(crate) fn directory_reference_target(&self) -> Option<&super::super::references::Target> {
        let Entity::Input(target) = &self.management.dialog.as_ref()?.target.entity else {
            return None;
        };
        Some(target)
    }
    pub(crate) fn open_directory_reference(&mut self, input: super::super::references::Target) {
        let ConnectionState::Connected { root_id, epoch } = &self.connection else {
            return;
        };
        let target = Target {
            root: root_id.clone(),
            epoch: epoch.clone(),
            name: String::new(),
            entity: Entity::Input(input),
        };
        self.apply(Action::Manage(Command::Open(target, Kind::Reference)));
    }
    pub(super) fn reference_can_select(&self) -> bool {
        let Some(dialog) = &self.management.dialog else {
            return false;
        };
        let Entity::Input(target) = &dialog.target.entity else {
            return false;
        };
        dialog.visible
            && !dialog.blocked
            && self.management_identity(&dialog.target)
            && self.reference_editable(target)
            && self.reference_count(target) < 4
            && dialog.browser.as_ref().is_some_and(|b| b.can_register())
    }
    pub(super) fn resolve_directory_reference(&mut self) {
        let Some(browser) = self
            .management
            .dialog
            .as_mut()
            .and_then(|d| d.browser.as_mut())
        else {
            return;
        };
        browser.resolving = true;
        browser.requested = true;
        browser.error = false;
    }
    pub(super) fn directory_reference_completed(
        &mut self,
        query: &Query,
        result: Result<QueryResult, String>,
    ) {
        let Some(target) = self.directory_reference_target().cloned() else {
            return;
        };
        let valid = match result {
            Ok(QueryResult::DirectoryPath {
                root_id,
                segments,
                path,
            }) if matches!(query, Query::DirectoryResolve {root_id:expected,segments:selected} if *expected==root_id && *selected==segments) => {
                Some(path)
            }
            _ => None,
        };
        if let Some(path) = valid
            && self.reference_editable(&target)
            && self.reference_count(&target) < 4
        {
            let root = self.management.dialog.as_ref().unwrap().target.root.clone();
            let item = DirectoryReference {
                host_id: root,
                path,
            };
            if super::super::references::validate(std::slice::from_ref(&item), &item.host_id)
                .is_ok()
                && let Some(items) = self.reference_items_mut(&target)
            {
                if !items.contains(&item) {
                    items.push(item);
                }
                self.management.dialog = None;
                self.hits.clear();
                return;
            }
        }
        if let Some(browser) = self
            .management
            .dialog
            .as_mut()
            .and_then(|d| d.browser.as_mut())
        {
            browser.resolving = false;
            browser.error = true;
        }
    }
}
