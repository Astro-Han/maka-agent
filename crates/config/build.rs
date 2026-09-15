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

fn main() {
    for source in [
        "migrations",
        "../../scripts/rust/generate-catalog-facts.mjs",
        "../../scripts/rust/catalog-facts-entry.mjs",
        "../../packages/core/src",
        "../../scripts/sync-model-metadata.mjs",
        "../../scripts/model-metadata/models-dev-api.snapshot.json",
        "../../package-lock.json",
    ] {
        println!("cargo:rerun-if-changed={source}");
    }
    println!("cargo:rerun-if-env-changed=MAKA_JS_DEPS");
    let dependencies = std::env::var_os("MAKA_JS_DEPS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "../..".into());
    for source in ["package-lock.json", "node_modules/.package-lock.json"] {
        println!(
            "cargo:rerun-if-changed={}",
            dependencies.join(source).display()
        );
    }
    let status = std::process::Command::new("node")
        .arg("../../scripts/rust/generate-catalog-facts.mjs")
        .arg(std::env::var("OUT_DIR").expect("Cargo OUT_DIR"))
        .status()
        .expect("Node is required to generate model facts at build time");
    assert!(
        status.success(),
        "model fact generation failed; install repository npm dependencies first"
    );
}
