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

use crate::{ConfigError, Result};
use maka_runtime::configuration::ConnectionCatalogEntry;

pub(crate) fn validate_overrides(row: &ConnectionCatalogEntry) -> Result<()> {
    if let Some(overrides) = &row.model_overrides {
        for id in overrides.keys() {
            let declaration = &overrides[id];
            let reported = row.models.iter().find(|model| model.id == *id);
            let context = declaration
                .context_window
                .or(reported.and_then(|m| m.context_window));
            let input = declaration
                .input_limit
                .or(reported.and_then(|m| m.input_limit));
            if matches!((context, input), (Some(context), Some(input)) if input > context) {
                return Err(ConfigError::Invalid(
                    "Model input limit exceeds the context window.".into(),
                ));
            }
        }
    }
    Ok(())
}
