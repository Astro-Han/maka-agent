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
    bundle_providers();
    for source in [
        "terminal.js",
        "TERMINAL_SOURCE.md",
        "../../scripts/rust/bundle-terminal.mjs",
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
        .arg("../../scripts/rust/bundle-terminal.mjs")
        .arg(std::env::var_os("OUT_DIR").expect("Cargo OUT_DIR"))
        .status()
        .expect("Node is required to bundle the terminal engine at build time");
    assert!(
        status.success(),
        "terminal bundle failed; install repository npm dependencies first"
    );
}

fn bundle_providers() {
    println!("cargo:rerun-if-changed=trusted/open-responses.js");
    println!("cargo:rerun-if-changed=trusted/adapter.js");
    println!("cargo:rerun-if-changed=trusted/codex-auth.js");
    println!("cargo:rerun-if-changed=trusted/provider-errors.js");
    println!("cargo:rerun-if-changed=trusted/provider-fetch.js");
    println!("cargo:rerun-if-changed=trusted/compatible-transport.js");
    println!("cargo:rerun-if-changed=trusted/responses-transport.js");
    println!("cargo:rerun-if-changed=trusted/network-fetch.js");
    println!("cargo:rerun-if-changed=../../scripts/rust/bundle-providers.mjs");
    println!("cargo:rerun-if-changed=third-party/deno-telemetry/telemetry.ts");
    println!("cargo:rerun-if-changed=third-party/deno-telemetry/util.ts");
    let status = std::process::Command::new("node")
        .arg("../../scripts/rust/bundle-providers.mjs")
        .arg(std::env::var("OUT_DIR").expect("Cargo OUT_DIR"))
        .stdin(std::process::Stdio::null())
        .status()
        .expect("Node is required to bundle the provider SDKs at build time");
    assert!(
        status.success(),
        "provider bundle failed; install repository npm dependencies first"
    );
}
