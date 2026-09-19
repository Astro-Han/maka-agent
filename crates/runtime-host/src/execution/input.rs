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

use super::skills::{FrozenSkills, SkillPreparation};
use super::{Result, failure, internal};
use maka_plugins::{
    composition::Scope,
    contributions::Catalog,
    input::{Prepared, Request},
};
use maka_protocol::OperationErrorCode;
mod prepared;
pub(crate) use prepared::PreparedMessageInput;

/// Compatibility projection for the existing Skill receipt fields. The execution
/// pipeline itself dispatches every published input provider without domain names.
pub(super) async fn prepare(
    catalog: &Catalog,
    mut request: Request,
) -> Result<(Prepared, SkillPreparation)> {
    let scope = Scope::Session(request.session_id.clone());
    let skills = catalog
        .snapshot::<maka_plugins::input::InputPreparation>(&scope)
        .entries
        .contains_key(maka_skills::plugin::ID);
    if !skills {
        let ids = request
            .selections
            .remove(maka_skills::plugin::ID)
            .unwrap_or_default();
        if let SkillPreparation::Blocked(receipt) = FrozenSkills::empty()
            .prepare(&mut request.content, &ids)
            .map_err(super::skills::skill_error)?
        {
            return Ok((
                Prepared::unchanged(request.content),
                SkillPreparation::Blocked(receipt),
            ));
        }
    }
    let prepared = maka_plugins::input::prepare(catalog, &scope, request)
        .await
        .map_err(internal)?;
    let receipt = prepared
        .content
        .preparation
        .iter()
        .rev()
        .find(|receipt| {
            receipt.source.name == maka_skills::plugin::ID
                && receipt.source.package_id == maka_skills::plugin::ID
        })
        .map(|receipt| {
            serde_json::from_value::<maka_runtime::skills::SkillInvocationResult>(
                receipt.receipt.clone(),
            )
        })
        .transpose()
        .map_err(internal)?
        .unwrap_or_default();
    if let Some(message) = &prepared.blocked {
        if !receipt.failed.is_empty() {
            return Ok((prepared, SkillPreparation::Blocked(receipt)));
        }
        return Err(failure(OperationErrorCode::OperationUnavailable, message));
    }
    let selection = SkillPreparation::Ready {
        skill_invocation: receipt,
        required_tools: prepared.required_tools.clone(),
    };
    Ok((prepared, selection))
}
