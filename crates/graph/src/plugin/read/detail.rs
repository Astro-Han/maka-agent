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
    WorkId,
    schedule::{HistoricalInput, Target, Work},
};
use maka_plugins::remote::Error;
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::plugin) struct Detail {
    work_id: WorkId,
    target: Target,
    instruction: String,
    offset: usize,
    total_bytes: usize,
    next_offset: Option<usize>,
    input_ids: Vec<String>,
    selected_result_inputs: Vec<HistoricalInput>,
    replaces: Option<String>,
}

pub(super) fn page(work: Work, offset: usize) -> Result<Detail, Error> {
    let total = work.instruction.len();
    if offset > total || !work.instruction.is_char_boundary(offset) {
        return Err(Error::Invalid(
            "Invalid Graph instruction byte cursor".into(),
        ));
    }
    let mut end = work
        .instruction
        .floor_char_boundary((offset + 8192).min(total));
    let mut detail = Detail {
        work_id: work.work_id,
        target: work.target,
        instruction: work.instruction[offset..end].into(),
        offset,
        total_bytes: total,
        next_offset: (end < total).then_some(end),
        input_ids: work.input_ids,
        selected_result_inputs: work.selected_result_inputs,
        replaces: work.replaces,
    };
    // JSON escapes and long input identities count toward the Remote envelope.
    while serde_json::to_vec(&detail).map_err(super::failure)?.len() > 48 * 1024 {
        end = work
            .instruction
            .floor_char_boundary(offset + (end - offset) / 2);
        if end == offset {
            return Err(Error::Invalid(
                "Graph work metadata exceeds read budget".into(),
            ));
        }
        detail.instruction.truncate(end - offset);
        detail.next_offset = Some(end);
    }
    Ok(detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn escaped_unicode_pages_recover_exact_instructions_with_all_input_references() {
        let instruction = "中\n\t\\\"".repeat(8000);
        let work = Work {
            work_id: WorkId::new(),
            target: Target::Agent {
                agent_id: "general".into(),
            },
            instruction: instruction.clone(),
            input_ids: (0..64)
                .map(|index| format!("{index}-{}", "\"".repeat(253)))
                .collect(),
            selected_result_inputs: vec![],
            replaces: None,
        };
        let mut offset = 0;
        let mut recovered = String::new();
        loop {
            let value = page(work.clone(), offset).unwrap();
            assert!(serde_json::to_vec(&value).unwrap().len() <= 48 * 1024);
            assert_eq!(value.input_ids, work.input_ids);
            assert_eq!(value.total_bytes, instruction.len());
            recovered.push_str(&value.instruction);
            match value.next_offset {
                Some(next) => {
                    assert!(next > offset);
                    offset = next;
                }
                None => break,
            }
        }
        assert_eq!(recovered, instruction);
        assert!(page(work, 1).is_err());
    }
}
