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

use maka_skills::{
    Catalog, DiscoveredSkill, DiscoverySnapshot, HostCapabilities, Preference, Preferences,
    PreparedInvocation, SkillLocation, inline_references, parse,
};
use maka_skills::{SkillInvocationResult, SkillScope, SkillSource};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

fn skill(id: &str, name: &str, body: &str) -> DiscoveredSkill {
    DiscoveredSkill {
        location: SkillLocation {
            reference: format!("project:maka:{id}"),
            id: id.into(),
            path: Path::new("/fixture/skills").join(id),
            discovery_root: "/fixture".into(),
            scope: SkillScope::Project,
            source: SkillSource::Maka,
            precedence: 0,
        },
        document: parse(&format!(
            "---\nname: '{}'\ndescription: work carefully\n---\n{body}",
            name.replace('\'', "''")
        ))
        .unwrap(),
        content_sha256: format!("sha256:{}", "0".repeat(64)),
        shadowed_by: None,
    }
}

fn prepare(catalog: &Catalog<'_>, text: &str, ids: &[String]) -> Value {
    let (result, projected) = match catalog.prepare_invocation(text, ids) {
        PreparedInvocation::Passthrough => {
            let result = SkillInvocationResult::default();
            let projected =
                json!({"disposition":"passthrough", "sendText":text, "skillInvocation":result});
            (result, projected)
        }
        PreparedInvocation::Ready { text, result, .. } => {
            let projected =
                json!({"disposition":"ready", "sendText":text, "skillInvocation":result});
            (result, projected)
        }
        PreparedInvocation::Blocked(result) => {
            let projected = json!({"disposition":"blocked", "skillInvocation":result});
            (result, projected)
        }
    };
    result.validate().unwrap();
    let roundtrip: SkillInvocationResult =
        serde_json::from_value(serde_json::to_value(&result).unwrap()).unwrap();
    assert_eq!(roundtrip, result);
    json!({"prepared":projected,"references":inline_references(&result.receipts, text)})
}

#[test]
fn execution_fingerprint_covers_frozen_content_and_resolution_inputs() {
    let mut discovery = DiscoverySnapshot {
        inventory: vec![skill("review", "Review", "old instructions")],
        rejected: Vec::new(),
        diagnostics: Vec::new(),
    };
    let mut preferences = Preferences::Available(BTreeMap::new());
    let mut host = HostCapabilities {
        tools: ["Read".into(), "Shell".into()].into(),
        capabilities: ["network".into(), "files".into()].into(),
    };
    let digest =
        |discovery: &DiscoverySnapshot, preferences: &Preferences, host: &HostCapabilities| {
            Catalog {
                discovery,
                preferences,
                host,
            }
            .fingerprint()
            .unwrap()
        };
    let original = digest(&discovery, &preferences, &host);
    host.tools = ["Shell".into(), "Read".into()].into();
    host.capabilities = ["files".into(), "network".into()].into();
    assert_eq!(digest(&discovery, &preferences, &host), original);
    assert_ne!(
        digest(&discovery, &Preferences::Unavailable, &host),
        original
    );

    // Discovery computes this digest over the complete SKILL.md bytes.
    discovery.inventory[0].document.body = "new instructions".into();
    discovery.inventory[0].content_sha256 =
        maka_runtime::artifact::content_digest(b"new SKILL.md bytes");
    let content = digest(&discovery, &preferences, &host);
    assert_ne!(
        content, original,
        "unchanged tool schemas cannot hide changed instructions"
    );
    discovery.inventory[0].location.path = "/moved/review".into();
    let moved = digest(&discovery, &preferences, &host);
    assert_ne!(
        moved, content,
        "relative references depend on the discovered path"
    );
    preferences = Preferences::Available(BTreeMap::from([(
        "project:maka:review".into(),
        Preference {
            enabled: false,
            pinned: false,
        },
    )]));
    let disabled = digest(&discovery, &preferences, &host);
    assert_ne!(disabled, moved);
    host.capabilities.remove("network");
    assert_ne!(digest(&discovery, &preferences, &host), disabled);
}

#[test]
fn explicit_preparation_and_receipt_wire_match_current_source() {
    let mut discovery = DiscoverySnapshot {
        inventory: vec![
            skill(
                "review",
                "Review <code>&",
                "Use the original snapshot.\nSecond line.",
            ),
            skill("unicode", &"😀".repeat(64), &"𐐀".repeat(24_001)),
            skill("disabled", "Disabled", "do not load"),
            skill("network", "Network", "requires unavailable capability"),
        ],
        rejected: Vec::new(),
        diagnostics: Vec::new(),
    };
    discovery.inventory[3]
        .document
        .manifest
        .attributes
        .required_capabilities
        .push("network".into());
    let large_refs = (0..50)
        .map(|i| {
            let mut entry = skill(
                &format!("{}{i:03}", "x".repeat(125)),
                &"😀".repeat(64),
                "body",
            );
            entry.location.reference = format!("{}:{}", "r".repeat(383), entry.location.id);
            let reference = entry.location.reference.clone();
            discovery.inventory.push(entry);
            reference
        })
        .collect::<Vec<_>>();
    let preferences = Preferences::Available(BTreeMap::from([(
        "project:maka:disabled".into(),
        Preference {
            enabled: false,
            pinned: false,
        },
    )]));
    let host = HostCapabilities::default();
    let catalog = Catalog {
        discovery: &discovery,
        preferences: &preferences,
        host: &host,
    };
    let mut cases = vec![
        json!({"text":"  untouched\n    indented", "ids":[]}),
        json!({"text":"😀 /skill:review\n    indented  code\n/skill:REVIEW /skill:missing", "ids":["project:maka:review","review"]}),
        json!({"text":"/skill:review", "ids":[]}),
        json!({"text":"/skill:disabled /skill:network /skill:missing", "ids":[]}),
        json!({"text":"after ids", "ids":["unicode"]}),
        json!({"text":"", "ids":[""," \u{feff} ","\u{0000}\u{007f}","missing","😀"]}),
        json!({"text":"/skill:review", "ids":["a".repeat(513)]}),
        json!({"text":"https://x/skill:review a/skill:review /skill:", "ids":[]}),
        json!({"text":"/skill:review /skill:Review /skill:review\n /skill:missing", "ids":["review"]}),
    ];
    for whitespace in [
        " ", "\t", "\r", "\n", "\u{feff}", "\u{2028}", "\u{2029}", "\u{0085}", "\u{200b}",
    ] {
        cases.push(json!({"text":format!("😀{whitespace}/skill:review trailing"), "ids":[]}));
    }
    for suffix in [":extra", "中文", "/tail", ".", "-x", "_x", "?q=x"] {
        cases.push(json!({"text":format!("/skill:review{suffix}"), "ids":[]}));
    }
    for count in [50, 51] {
        cases.push(json!({"text":"no provider call", "ids":(0..count).map(|i|format!("missing-{i}")).collect::<Vec<_>>()}));
    }
    cases.push(json!({"text":"receipts must fit before a provider turn", "ids":large_refs}));
    let expected: Vec<Value> = cases
        .iter()
        .map(|case| {
            let ids = serde_json::from_value::<Vec<String>>(case["ids"].clone()).unwrap();
            prepare(&catalog, case["text"].as_str().unwrap(), &ids)
        })
        .collect();
    let inventory: Vec<Value> = discovery.inventory.iter().map(|skill| json!({
        "ref":skill.location.reference, "id":skill.location.id,
        "name":skill.document.manifest.name, "description":skill.document.manifest.description,
        "content":skill.document.body, "path":skill.location.path,
        "discoveryRoot":skill.location.discovery_root, "scope":skill.location.scope,
        "source":skill.location.source, "precedence":skill.location.precedence,
        "enabled":skill.location.id != "disabled", "pinned":false,
        "declaredTools":skill.document.manifest.attributes.allowed_tools,
        "requiredTools":skill.document.manifest.attributes.required_tools,
        "requiredCapabilities":skill.document.manifest.attributes.required_capabilities,
    })).collect();

    let loaded = expected[2]["prepared"]["skillInvocation"]["receipts"][0].clone();
    let failed =
        json!({"invocation":"model_tool","request":"missing","success":false,"reason":"not_found"});
    let overflow = json!({"invocation":"explicit","success":false,"reason":"too_many_requests","requestLimit":50});
    let mut receipts = vec![loaded.clone(), failed.clone(), overflow.clone()];
    for (receipt, field, values) in [
        (
            loaded,
            "success",
            vec![json!(false), json!("true"), json!(1), Value::Null],
        ),
        (failed, "success", vec![json!(true)]),
        (overflow.clone(), "invocation", vec![json!("model_tool")]),
        (
            overflow,
            "requestLimit",
            vec![json!(0), json!(51), json!(1.5)],
        ),
    ] {
        for value in values {
            let mut altered = receipt.clone();
            altered[field] = value;
            receipts.push(altered);
        }
        let mut extra = receipt;
        extra["unexpected"] = json!(true);
        receipts.push(extra);
    }
    let accepted: Vec<bool> = receipts
        .iter()
        .map(|receipt| {
            serde_json::from_value::<SkillInvocationResult>(
                json!({"loaded":[],"failed":[],"receipts":[receipt]}),
            )
            .is_ok_and(|result| result.validate().is_ok())
        })
        .collect();
    let script = r#"
        import { readFileSync } from 'node:fs';
        import { withSourceModule } from './tests/support/source.mjs';
        const {inventory,cases,receipts} = JSON.parse(readFileSync(0,'utf8'));
        await withSourceModule('packages/runtime/src/skill-invocation.ts', async (invocation) => {
          await withSourceModule('packages/runtime/src/skill-invocation-receipt.ts', async (receipt) => {
            await withSourceModule('packages/core/src/skill-invocation.ts', async (codec) => {
              const prepared = [];
              for (const c of cases) {
                const result = await invocation.prepareSkillInvocationMessageFromInventory({
                  text:c.text, skillIds:c.ids, inventory, host:{toolNames:new Set()}
                });
                prepared.push({prepared:result,references:receipt.skillInvocationInlineReferences(result.skillInvocation.receipts,c.text)});
              }
              const accepted = receipts.map(r => {
                try { codec.decodeSkillInvocationResult({loaded:[],failed:[],receipts:[r]}); return true; }
                catch { return false; }
              });
              process.stdout.write(JSON.stringify({prepared,accepted}));
            });
          });
        });
    "#;
    let mut child = Command::new("node")
        .args(["--input-type=module", "-e", script])
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            &serde_json::to_vec(&json!({
                "inventory":inventory,"cases":cases,"receipts":receipts
            }))
            .unwrap(),
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let actual: Value = serde_json::from_slice(&output.stdout).unwrap();
    for (index, expected) in expected.iter().enumerate() {
        assert_eq!(
            expected, &actual["prepared"][index],
            "case {}",
            cases[index]
        );
    }
    assert_eq!(json!(accepted), actual["accepted"]);
    discovery.inventory[0]
        .document
        .manifest
        .attributes
        .required_tools = vec!["Read".into(), "Read".into()];
    discovery.inventory[2]
        .document
        .manifest
        .attributes
        .required_tools = vec!["Bash".into()];
    let host = HostCapabilities {
        tools: ["Read".into()].into(),
        ..Default::default()
    };
    let catalog = Catalog {
        discovery: &discovery,
        preferences: &preferences,
        host: &host,
    };
    let PreparedInvocation::Ready {
        required_tools,
        result,
        ..
    } = catalog.prepare_invocation(
        "/skill:review /skill:review /skill:disabled /skill:network",
        &[],
    )
    else {
        panic!("partial loading must preserve successful Skills");
    };
    assert_eq!(
        required_tools,
        ["Read".into()].into(),
        "only successful, deduplicated prerequisites survive"
    );
    assert_eq!(result.loaded.len(), 1);
    assert_eq!(result.failed.len(), 2);
}
