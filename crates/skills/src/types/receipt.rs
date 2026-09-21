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

use super::{SkillFailureReason, SkillInvocationMode, SkillScope, SkillSource, TooManyRequests};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error, ser::SerializeMap};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillInvocationReceipt {
    Loaded(SkillLoadedReceipt),
    Failed(SkillFailedReceipt),
    /// Request overflow can only describe an explicit, failed invocation.
    Overflow {
        request_limit: u64,
    },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillLoadedReceipt {
    pub invocation: SkillInvocationMode,
    pub request: String,
    pub skill_ref: String,
    pub id: String,
    pub name: String,
    pub scope: SkillScope,
    pub source: SkillSource,
    pub truncated: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillFailedReceipt {
    pub invocation: SkillInvocationMode,
    pub request: String,
    pub reason: SkillFailureReason,
}

impl Serialize for SkillInvocationReceipt {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(match self {
            Self::Loaded(_) => 9,
            Self::Failed(_) | Self::Overflow { .. } => 4,
        }))?;
        match self {
            Self::Loaded(r) => {
                map.serialize_entry("invocation", &r.invocation)?;
                map.serialize_entry("request", &r.request)?;
                map.serialize_entry("success", &true)?;
                map.serialize_entry("ref", &r.skill_ref)?;
                map.serialize_entry("id", &r.id)?;
                map.serialize_entry("name", &r.name)?;
                map.serialize_entry("scope", &r.scope)?;
                map.serialize_entry("source", &r.source)?;
                map.serialize_entry("truncated", &r.truncated)?;
            }
            Self::Failed(r) => {
                map.serialize_entry("invocation", &r.invocation)?;
                map.serialize_entry("request", &r.request)?;
                map.serialize_entry("success", &false)?;
                map.serialize_entry("reason", &r.reason)?;
            }
            Self::Overflow { request_limit } => {
                map.serialize_entry("invocation", &SkillInvocationMode::Explicit)?;
                map.serialize_entry("success", &false)?;
                map.serialize_entry("reason", &TooManyRequests::TooManyRequests)?;
                map.serialize_entry("requestLimit", request_limit)?;
            }
        }
        map.end()
    }
}

/// Boolean wire discriminants are validated here, not retained as mutable
/// business fields beside a contradictory enum variant.
#[derive(Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum WireReceipt {
    Loaded {
        invocation: SkillInvocationMode,
        request: String,
        success: bool,
        #[serde(rename = "ref")]
        skill_ref: String,
        id: String,
        name: String,
        scope: SkillScope,
        source: SkillSource,
        truncated: bool,
    },
    Failed {
        invocation: SkillInvocationMode,
        request: String,
        success: bool,
        reason: SkillFailureReason,
    },
    Overflow {
        invocation: SkillInvocationMode,
        success: bool,
        reason: TooManyRequests,
        #[serde(rename = "requestLimit")]
        request_limit: u64,
    },
}

impl<'de> Deserialize<'de> for SkillInvocationReceipt {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match WireReceipt::deserialize(deserializer)? {
            WireReceipt::Loaded {
                invocation,
                request,
                success: true,
                skill_ref,
                id,
                name,
                scope,
                source,
                truncated,
            } => Ok(Self::Loaded(SkillLoadedReceipt {
                invocation,
                request,
                skill_ref,
                id,
                name,
                scope,
                source,
                truncated,
            })),
            WireReceipt::Failed {
                invocation,
                request,
                success: false,
                reason,
            } => Ok(Self::Failed(SkillFailedReceipt {
                invocation,
                request,
                reason,
            })),
            WireReceipt::Overflow {
                invocation: SkillInvocationMode::Explicit,
                success: false,
                reason: TooManyRequests::TooManyRequests,
                request_limit,
            } => Ok(Self::Overflow { request_limit }),
            _ => Err(D::Error::custom("contradictory skill receipt outcome")),
        }
    }
}
