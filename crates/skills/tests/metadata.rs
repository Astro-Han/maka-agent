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

use maka_skills::parse;
use serde_json::{Value, json};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

fn projection(text: &str) -> Value {
    match parse(text) {
        Ok(document) => {
            json!({"valid":true,"manifest":document.manifest,"body":document.body,"issues":document.issues})
        }
        Err(document) => {
            json!({"valid":false,"manifest":document.manifest,"body":document.body,"issues":document.issues})
        }
    }
}

#[test]
fn current_metadata_source_preserves_requirements_yaml_and_unicode_semantics() {
    let mut cases = vec![
        "\u{feff}--- \r\nname: code-review\r\ndescription: |\r\n  Review carefully.\r\n  Keep the design.\r\nallowed-tools: Read, Shell Read\r\nmetadata: {owner: Maka}\r\n---\r\n\r\n  Instructions 😀\r\nsecond line\r\n".to_owned(),
        "---\nname: '&<>quoted'\ndescription: yes\nlicense: Apache-2.0\ncompatibility: Linux\ncategory: development\n---\nbody".into(),
        "no frontmatter\nkeep body".into(),
        "---\nname: never closed".into(),
        "---\n[]\n---\nbody".into(),
        "---\nname: same\nname: duplicate\ndescription: invalid\n---\nbody".into(),
        "---\nname: &name shared\ndescription: *name\n---\nbody".into(),
        "---\nname: !unknown opaque\ndescription: no tag extension\n---\nbody".into(),
        "---\nname: <<\ndescription: normal\n<<: {other: value}\n---\nbody".into(),
        "---\nname: safe\ndescription: safe\nmetadata: {x: [y], z: '\u{feff} string \u{feff}'}\nunknown: 1\n---\nbody".into(),
    ];
    for field in [
        "name",
        "description",
        "allowed-tools",
        "required-tools",
        "required-capabilities",
        "license",
        "compatibility",
        "category",
        "metadata",
    ] {
        for value in [
            "null",
            "Null",
            "NULL",
            "nUlL",
            ".nan",
            ".inf",
            "1e999",
            "!!str null",
            "!!str ~",
            "!!null Read",
            "!!bool 'true'",
            "!!int '123'",
            "!!float '1.2'",
            "!!float '1'",
            "!!null 'NULL'",
            "false",
            "True",
            "NO",
            "123",
            "01",
            "0xffffffffffffffff",
            "0o1777777777777777777777",
            "1.2",
            "''",
            "'  '",
            "'a  b,a'",
            "',a,'",
            "[]",
            "[Read, Read, 'Shell Run', 42, '']",
            "{k: v}",
        ] {
            let required = match field {
                "name" => "description: description\n".to_owned(),
                "description" => "name: name\n".to_owned(),
                _ => "name: name\ndescription: description\n".to_owned(),
            };
            cases.push(format!("---\n{required}{field}: {value}\n---\nbody"));
        }
    }
    for (field, size) in [("name", 65), ("description", 1025), ("compatibility", 501)] {
        let required = match field {
            "name" => "description: valid\n",
            "description" => "name: valid\n",
            _ => "name: valid\ndescription: valid\n",
        };
        cases.push(format!(
            "---\n{required}{field}: {}\n---\n{}",
            "😀".repeat(size),
            "😀".repeat(24_001)
        ));
    }
    cases.push("---\nname: '\u{0085}'\ndescription: '\u{feff}'\n---\nbody".into());

    let expected: Vec<Value> = cases.iter().map(|text| projection(text)).collect();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script = r#"
        import { readFileSync } from 'node:fs';
        import { withSourceModule } from './tests/support/source.mjs';
        const cases = JSON.parse(readFileSync(0, 'utf8'));
        await withSourceModule('packages/runtime/src/skills-metadata.ts', ({validateSkillMetadata}) => {
            process.stdout.write(JSON.stringify(cases.map(validateSkillMetadata)));
        });
    "#;
    let mut child = Command::new("node")
        .args(["--input-type=module", "-e", script])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&cases).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let actual: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(actual.len(), expected.len());
    for ((text, expected), actual) in cases.iter().zip(expected).zip(actual) {
        assert_eq!(expected, actual, "SKILL.md: {text}");
    }
}
