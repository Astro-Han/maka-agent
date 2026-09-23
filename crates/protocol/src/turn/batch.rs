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

/// One ordered input of an atomic Turn. Source identities belong to the new
/// opening, not to the historical messages used to compose this request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TurnStartMessage {
    pub content: MessageContent,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub input_selections: maka_runtime::input::Selections,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TurnBatchStartInput {
    pub session_id: String,
    pub turn_id: String,
    pub messages: Vec<TurnStartMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_orchestration: Option<TurnOrchestration>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<u64>,
}

pub fn decode_turn_batch_start_input(value: &Value) -> Result<TurnBatchStartInput> {
    validate_batch(decode(value)?, &[])
}

/// Preflight a client draft whose local attachments do not have Host references yet.
/// This never authorizes admission: the complete request must pass the ordinary decoder.
pub fn validate_turn_batch_draft(
    input: TurnBatchStartInput,
    pending_attachments: &[usize],
) -> Result<TurnBatchStartInput> {
    ensure(
        pending_attachments.len() == input.messages.len(),
        "Invalid pending attachment counts",
    )?;
    validate_batch(input, pending_attachments)
}

fn validate_batch(
    mut input: TurnBatchStartInput,
    pending: &[usize],
) -> Result<TurnBatchStartInput> {
    entity(&input.session_id)?;
    entity(&input.turn_id)?;
    ensure(
        (1..=64).contains(&input.messages.len()),
        "Invalid Turn input count",
    )?;
    if let Some(max) = input.max_steps {
        ensure(max > 0, "Invalid maxSteps")?;
    }
    let mut references = 0;
    let mut allow_empty = false;
    let mut contents = Vec::with_capacity(input.messages.len());
    let mut attachments = 0usize;
    for (index, message) in input.messages.iter_mut().enumerate() {
        let count = pending.get(index).copied().unwrap_or(0);
        ensure(count <= 8, "Too many pending attachments")?;
        attachments += count + message.content.attachments.iter().flatten().count();
        maka_runtime::input::validate_selections(&message.input_selections)
            .map_err(ProtocolError::invalid)?;
        let selected = !message.input_selections.is_empty() || count > 0;
        message.content.validate_admission(selected)?;
        allow_empty |= selected;
        references += message
            .content
            .inline_references
            .as_ref()
            .map_or(0, Vec::len);
        contents.push(maka_runtime::input::MessageInput::from(
            message.content.clone(),
        ));
    }
    ensure(attachments <= 8, "Too many attachments across Turn inputs")?;
    // aggregate() bounds its display references; reject before it can omit any.
    ensure(
        references <= 32,
        "Too many inline references across Turn inputs",
    )?;
    let aggregate = maka_runtime::message::aggregate(&contents);
    ensure(
        aggregate.text_bytes() <= 64 * 1024,
        "Turn inputs exceed durable capacity",
    )?;
    MessageContent::from(aggregate).validate_admission(allow_empty)?;
    // Leave room in the 700 KiB source-query envelope for Host-generated
    // message IDs and the per-source copy of the shared orchestration intent.
    encoded(&input, 640 * 1024)?;
    Ok(input)
}

pub fn assert_batch_start_output_for_input(
    input: &TurnBatchStartInput,
    output: &TurnStartResult,
) -> Result<()> {
    if let TurnStartResult::Started { turn, .. } = output {
        ensure(
            input.session_id == turn.session_id && input.turn_id == turn.turn_id,
            "Turn batch start changed operation identity",
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn batches_preserve_inputs_and_reject_total_limits_before_reference_aggregation() {
        let message = json!({"content":{"text":"🦀 @a.rs","inlineReferences":[
            {"kind":"workspace_file","value":"@a.rs","label":"a.rs","start":3}]}});
        let mut value =
            json!({"sessionId":"s","turnId":"t","messages":[message.clone(),message.clone()]});
        let input = decode_turn_batch_start_input(&value).unwrap();
        assert_eq!(input.messages.len(), 2);
        assert_eq!(
            input.messages[1]
                .content
                .inline_references
                .as_ref()
                .unwrap()[0]
                .start,
            3
        );
        let contents: Vec<_> = input
            .messages
            .into_iter()
            .map(|m| maka_runtime::input::MessageInput::from(m.content))
            .collect();
        let combined = maka_runtime::message::aggregate(&contents);
        assert_eq!(combined.inline_references.unwrap()[1].start, 13);
        for count in [0, 33, 65] {
            value["messages"] = json!(vec![message.clone(); count]);
            assert!(decode_turn_batch_start_input(&value).is_err());
        }
        value["messages"] = json!([{"content":{"text":"a".repeat(30_000)}},{"content":{"text":"b".repeat(30_000)}}]);
        assert!(
            decode_turn_batch_start_input(&value).is_err(),
            "a batch does not multiply the per-Turn input budget"
        );
    }
    #[test]
    fn pending_attachment_preflight_never_relaxes_wire_admission() {
        let input: TurnBatchStartInput = serde_json::from_value(serde_json::json!({
            "sessionId":"target", "turnId":"turn",
            "messages":[{"content":{"text":""}},{"content":{"text":"second"}}]
        }))
        .unwrap();
        assert!(decode_turn_batch_start_input(&serde_json::to_value(&input).unwrap()).is_err());
        let checked = validate_turn_batch_draft(input.clone(), &[1, 0]).unwrap();
        assert_eq!(checked, input);
        for pending in [&[0, 1][..], &[1][..], &[8, 1][..], &[usize::MAX, 0][..]] {
            assert!(validate_turn_batch_draft(input.clone(), pending).is_err());
        }
        let mut oversized = input;
        oversized.messages[1].content.text = "x".repeat(48 * 1024 + 1);
        assert!(validate_turn_batch_draft(oversized, &[1, 0]).is_err());
    }
}
