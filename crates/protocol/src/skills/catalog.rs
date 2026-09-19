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

use super::{MAX_ITEMS, MAX_PAGE_BYTES, invalid, revision, text};
use crate::{Result, codec};
use serde_json::Value;

use super::{CatalogInput, CatalogItem, CatalogResult, CatalogView};
pub fn decode_catalog_input(value: &Value) -> Result<CatalogInput> {
    let input: CatalogInput = serde_json::from_value(value.clone()).map_err(invalid)?;
    super::validate_workspace(&input.context().workspace)?;
    if let CatalogInput::Continue {
        revision: r,
        cursor,
        ..
    } = &input
    {
        revision(r)?;
        text(cursor, 1024)?;
    }
    Ok(input)
}
pub fn decode_catalog_output(value: &Value) -> Result<CatalogResult> {
    if value["kind"] == "page" {
        codec::exact(
            codec::record(value, "Skill source page")?,
            &[
                "kind",
                "view",
                "revision",
                "items",
                "nextCursor",
                "resolvedWorkspace",
            ],
        )?;
    }
    let result: CatalogResult = serde_json::from_value(value.clone()).map_err(invalid)?;
    let workspace = match &result {
        CatalogResult::Page {
            view,
            revision: r,
            items,
            next_cursor,
            resolved_workspace,
        } => {
            revision(r)?;
            if items.len() > MAX_ITEMS {
                return Err(invalid("Too many skill source items"));
            }
            let workspace_overhead = ",\"resolvedWorkspace\":".len()
                + serde_json::to_vec(resolved_workspace)
                    .map_err(invalid)?
                    .len();
            if serde_json::to_vec(&result).map_err(invalid)?.len()
                > MAX_PAGE_BYTES + workspace_overhead
            {
                return Err(invalid("Skill source page exceeds byte limit"));
            }
            if let Some(cursor) = next_cursor {
                text(cursor, 1024)?;
            }
            for item in items {
                let (id, name, description, category) = match item {
                    CatalogItem::Skill(item) | CatalogItem::DiscoveryDiagnostic(item) => {
                        if *view != CatalogView::Governance {
                            return Err(invalid("Invalid governance page"));
                        }
                        super::governance::validate(item)?;
                        continue;
                    }
                    CatalogItem::Bundled {
                        id,
                        name,
                        description,
                        category,
                        declared_tools,
                        ..
                    } => {
                        if *view != CatalogView::Bundled || declared_tools.len() > 64 {
                            return Err(invalid("Invalid bundled page"));
                        }
                        for tool in declared_tools {
                            text(tool, 256)?;
                        }
                        (id, name, description, category)
                    }
                    CatalogItem::ManagedSource {
                        id,
                        name,
                        description,
                        category,
                        ..
                    } => {
                        if *view != CatalogView::ManagedSources {
                            return Err(invalid("Invalid managed source page"));
                        }
                        (id, name, description, category)
                    }
                };
                text(id, 81)?;
                if !id.as_bytes()[0].is_ascii_alphanumeric()
                    || !id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                {
                    return Err(invalid("Invalid source id"));
                }
                text(name, 256)?;
                text(category, 128)?;
                if description.len() > 4096 {
                    return Err(invalid("Invalid source description"));
                }
            }
            resolved_workspace
        }
        CatalogResult::RevisionChanged {
            expected_revision,
            actual_revision,
            resolved_workspace,
        } => {
            revision(expected_revision)?;
            revision(actual_revision)?;
            resolved_workspace
        }
    };
    super::validate_projection(workspace)?;
    Ok(result)
}
