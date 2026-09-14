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

use maka_runtime::{
    context::{
        MAX_SUMMARY_BYTES, SUMMARY_FORMAT_TEMPLATE, SummaryDefect, SummaryFormat, TextSummary,
        validate_summary,
    },
    model::{ModelFinishReason, ModelPart, ModelStep, ModelUsage, TextKind},
};
use std::{
    path::Path,
    process::{Command, Stdio},
};

const VALID: &str = "## Goal\nKeep durable evidence.\n## Progress\n### Done\n- Saved raw output.\n## Next Steps\n1. Verify replay.\n## Critical Context\n- src/event.rs and cargo nextest passed.";

#[test]
fn structural_scanner_matches_original_typescript_on_markdown_adversaries() {
    let mut cases = vec![
        VALID.into(),
        "".into(),
        SUMMARY_FORMAT_TEMPLATE.into(),
        format!("```markdown\n{VALID}\n```"),
        VALID.replace(
            "### Done\n- Saved raw output.",
            "## Key Decisions\n- Saved raw output.",
        ),
    ];
    for suffix in [
        "...", ":", "：", ",", "，", "、", ";", "；", "…", "(", "（", "—", "`", ".",
    ] {
        cases.push(format!("{VALID}{suffix}"));
    }
    for indent in 0..=5 {
        cases.push(
            VALID
                .lines()
                .map(|line| format!("{}{line}", " ".repeat(indent)))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        for family in ['`', '~'] {
            for width in [3, 4] {
                for closing in ["```", "````", "~~~", "~~~~", "``` trailing", "    ````"] {
                    let open =
                        format!("{}{}", " ".repeat(indent), family.to_string().repeat(width));
                    cases.push(format!(
                        "{VALID}\n{open}text\n## Goal\nliteral command error ``` details\n{closing}"
                    ));
                }
            }
        }
    }
    for body in [
        "",
        "### Done",
        "   ### Done",
        "    ### Done",
        "- - -",
        "***",
        "_ _ _",
        "- * -",
        "[What the user is trying to accomplish]",
        "```\n```",
        "```\ncommand\n```",
        "~~~\n```\n~~~",
        "(none)",
        "\u{85}",
        "\u{feff}",
    ] {
        cases.push(VALID.replace("- Saved raw output.", body));
    }
    cases.push(VALID.replace('\n', "\r\n"));
    cases.push(VALID.replace("## Progress", "## Progress\u{feff}"));
    cases.push(VALID.replace("## Progress", "## Progress\u{85}"));
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/runtime/src/history-compact-summary-validation.ts");
    let mut child = Command::new("node").args(["--input-type=module", "-e", r#"
import {readFileSync} from 'node:fs';
import {pathToFileURL} from 'node:url';
const {findCheckpointSummaryDefect} = await import(pathToFileURL(process.argv[1]));
const cases = JSON.parse(readFileSync(0, 'utf8'));
process.stdout.write(JSON.stringify(cases.map(text => text.trim().length === 0 ? 'empty_summary' : findCheckpointSummaryDefect(text) ?? null)));
"#]).arg(source).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    serde_json::to_writer(child.stdin.take().unwrap(), &cases).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let expected: Vec<Option<String>> = serde_json::from_slice(&output.stdout).unwrap();
    for (text, expected) in cases.iter().zip(expected) {
        assert_eq!(
            validate_summary(text, None)
                .err()
                .map(|error| error.to_string()),
            expected,
            "{text:?}"
        );
    }
}

fn step(text: &str) -> ModelStep {
    ModelStep {
        parts: vec![ModelPart::Text {
            text_kind: TextKind::Text,
            text: text.into(),
            provider_options: None,
        }],
        finish_reason: ModelFinishReason::Stop,
        usage: ModelUsage::default(),
        provider_options: None,
        response_id: None,
        model: None,
        timestamp: None,
    }
}

#[test]
fn summary_extraction_uses_complete_text_real_initial_usage_and_closed_shapes() {
    let (left, right) = VALID.split_at(15);
    let mut model = step(left);
    model.parts.push(ModelPart::Text {
        text_kind: TextKind::Text,
        text: right.into(),
        provider_options: None,
    });
    model.parts.insert(
        0,
        ModelPart::Text {
            text_kind: TextKind::Thinking,
            text: "hidden deliberation".into(),
            provider_options: None,
        },
    );
    let expected = TextSummary {
        format: SummaryFormat::SectionsV1,
        text: VALID.into(),
    };
    assert_eq!(
        serde_json::from_value::<TextSummary>(serde_json::to_value(&expected).unwrap()).unwrap(),
        expected
    );
    assert_eq!(
        TextSummary::from_model_step(&model, true).unwrap(),
        expected
    );
    model.usage.input_tokens = Some(10_001);
    assert_eq!(
        TextSummary::from_model_step(&model, true).unwrap(),
        expected
    );
    model.usage.output_tokens = Some(199);
    assert_eq!(
        TextSummary::from_model_step(&model, true),
        Err(SummaryDefect::TooSmallForFold)
    );
    assert_eq!(
        TextSummary::from_model_step(&model, false).unwrap(),
        expected
    );
    model.usage.output_tokens = Some(200);
    assert_eq!(
        TextSummary::from_model_step(&model, true).unwrap(),
        expected
    );
    model.finish_reason = ModelFinishReason::Length;
    assert_eq!(
        TextSummary::from_model_step(&model, true),
        Err(SummaryDefect::InvalidFinish)
    );
    model.finish_reason = ModelFinishReason::Stop;
    model.parts.push(ModelPart::ToolResult {
        id: "tool".into(),
        name: "Read".into(),
        output: serde_json::Value::Null,
        is_error: false,
        provider_options: None,
    });
    assert_eq!(
        TextSummary::from_model_step(&model, false),
        Err(SummaryDefect::ToolContent)
    );
    assert!(
        serde_json::from_value::<TextSummary>(
            serde_json::json!({"format":"future_format","text":VALID})
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<TextSummary>(
            serde_json::json!({"format":"sections_v1","text":VALID,"provider_state":"unexpected"})
        )
        .is_err()
    );
}

#[test]
fn summary_byte_limit_counts_utf8_and_never_truncates_a_replacement() {
    let mut text = VALID.to_owned();
    text.push_str(&"🦊".repeat((MAX_SUMMARY_BYTES - text.len()) / 4));
    text.push_str(&"x".repeat(MAX_SUMMARY_BYTES - text.len()));
    assert!(validate_summary(&text, None).is_ok());
    text.push('x');
    assert_eq!(validate_summary(&text, None), Err(SummaryDefect::TooLarge));
    assert_eq!(
        TextSummary::from_model_step(&step(&text), true),
        Err(SummaryDefect::TooLarge)
    );
}
