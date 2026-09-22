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

use crate::{CellDiagnostic, CellDiagnosticKind, ToolMetadata};
use deno_core::{JsRuntime, v8};
use serde::Deserialize;
use serde_json::Value;

pub(crate) async fn evaluate(
    runtime: &mut JsRuntime,
    source: &str,
    names: &[String],
    max_bytes: usize,
    metadata: &[ToolMetadata],
) -> Result<Value, CellDiagnostic> {
    let execution = |error: String| CellDiagnostic::new(CellDiagnosticKind::ExecutionError, error);
    let names = serde_json::to_string(names).unwrap();
    let metadata = serde_json::to_string(metadata).unwrap();
    runtime
        .execute_script(
            "maka:code/bootstrap",
            format!(
                r#"(() => {{
        const call = Deno.core.ops.op_maka_tool;
        const emit = Deno.core.ops.op_maka_emit;
        const yieldOutput = Deno.core.ops.op_maka_yield;
        const save = Deno.core.ops.op_maka_store;
        const read = Deno.core.ops.op_maka_load;
        const sleep = Deno.core.ops.op_maka_sleep;
        const diagnostics = new WeakMap();
        const remember = diagnostics.set.bind(diagnostics);
        const lookup = diagnostics.get.bind(diagnostics);
        const stringify = JSON.stringify;
        const describe = String;
        const hasOwn = Object.hasOwn;
        const ErrorClass = Error;
        const exitSignal = Object.freeze({{}});
        const check = (diagnostic) => {{
            if (!diagnostic) return;
            const error = new ErrorClass(diagnostic.message);
            remember(error, diagnostic);
            throw error;
        }};
        const text = (value) => {{
            const content = typeof value === "string" ? value : stringify(value);
            check(emit({{kind:"text",text:content === undefined ? "undefined" : content}}));
        }};
        const timers = new Map();
        let nextTimer = 0;
        let pendingTimers = 0;
        const media = (type, value) => {{
            if (typeof value === "string") {{
                const match = /^data:([^;,]+);base64,([A-Za-z0-9+/]*={{0,2}})$/.exec(value);
                if (!match || !match[1].startsWith(type + "/")) throw new ErrorClass("expected a base64 data URL");
                value = {{type, mimeType:match[1],data:match[2]}};
            }}
            if (!value || value.type !== type) throw new ErrorClass("expected a " + type + " content block");
            check(emit({{kind:"media",content:{{type,data:value.data,mimeType:value.mimeType}}}}));
        }};
        const helpers = {{
            text,
            exit: () => {{ throw exitSignal; }},
            image: (value) => {{
                if (!value?.ref) {{ media("image", value?.image_url ?? value); return; }}
                const {{mimeType, ref}} = value;
                check(emit({{kind:"image",image:{{mimeType,ref}}}}));
            }},
            audio: (value) => media("audio", value?.audio_url ?? value),
            generatedImage: (value) => {{ media("image", value.image_url); if (value.output_hint) text(value.output_hint); }},
            notify: (value) => {{ text(value); yieldOutput(); }},
            yield_control: async () => {{ yieldOutput(); await sleep(0); }},
            store: (key, value) => {{
                if (typeof key !== "string") throw new ErrorClass("store key must be a string");
                const json = stringify(value);
                if (json === undefined) throw new ErrorClass("stored value must be JSON");
                check(save(key, JSON.parse(json)));
            }},
            load: (key) => {{
                if (typeof key !== "string") throw new ErrorClass("load key must be a string");
                const result = read(key);
                return result.found ? result.value : undefined;
            }},
            setTimeout: (callback, millis = 0) => {{
                if (typeof callback !== "function" || !Number.isFinite(millis) || millis < 0 || millis > 86400000)
                    throw new ErrorClass("invalid timer");
                if (pendingTimers >= 128) throw new ErrorClass("timer limit exceeded");
                const id = ++nextTimer;
                pendingTimers++;
                timers.set(id, callback);
                sleep(Math.trunc(millis)).then(() => {{
                    pendingTimers--;
                    const callback = timers.get(id);
                    timers.delete(id);
                    if (callback) callback();
                }});
                return id;
            }},
            clearTimeout: (id) => {{ timers.delete(id); }},
            ALL_TOOLS: Object.freeze({metadata}.map(Object.freeze)),
        }};
        for (const [name, value] of Object.entries(helpers))
            Object.defineProperty(globalThis, name, {{value}});
        const invoke = async (name, input) => {{
            const outcome = await call(name, input);
            if (outcome.ok) return outcome.value;
            const error = new ErrorClass(outcome.error.message);
            remember(error, outcome.error);
            throw error;
        }};
        const catalog = Object.create(null);
        for (const name of {names}) {{
            Object.defineProperty(catalog, name, {{
                value: (input) => invoke(name, input), enumerable: true
            }});
        }}
        Object.defineProperty(globalThis, "tools", {{ value: new Proxy(Object.freeze(catalog), {{
            get(target, name) {{
                if (typeof name !== "string") return undefined;
                return hasOwn(target, name) ? target[name] : (input) => invoke(name, input);
            }}
        }}) }});
        globalThis.__maka_run = async (cell) => {{
            try {{
                const value = await cell();
                const json = stringify(value === undefined ? null : value);
                if (json === undefined) throw new ErrorClass("result is not JSON");
                return stringify({{kind: "success", json}});
            }} catch (error) {{
                if (error === exitSignal) return stringify({{kind:"success", json:"null"}});
                const diagnostic = lookup(error);
                return stringify({{kind: "failure", error: diagnostic ?? {{
                    kind: "execution_error", message: describe(error)
                }} }});
            }}
        }};
        delete globalThis.Deno;
    }})();"#
            ),
        )
        .map_err(|error| execution(error.to_string()))?;
    // Compile the user function without executing its body. A runtime SyntaxError
    // (including eval failures) is therefore distinct from this parse boundary.
    runtime
        .execute_script(
            "maka:code/cell",
            format!("globalThis.__maka_cell = async () => {{\n{source}\n}};"),
        )
        .map_err(|error| CellDiagnostic::new(CellDiagnosticKind::ParseError, error.to_string()))?;
    let value = runtime.execute_script("maka:code/run",
        "(() => { const run = __maka_run, cell = __maka_cell; delete globalThis.__maka_run; delete globalThis.__maka_cell; return run(cell); })()"
    ).map_err(|error| execution(error.to_string()))?;
    let resolving = runtime.resolve(value);
    let value = runtime
        .with_event_loop_promise(resolving, Default::default())
        .await
        .map_err(|error| execution(error.to_string()))?;
    let json = {
        deno_core::scope!(scope, runtime);
        let value = v8::Local::new(scope, value);
        deno_core::serde_v8::from_v8::<String>(scope, value)
            .map_err(|error| execution(error.to_string()))?
    };
    #[derive(Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
    enum Outcome {
        Success { json: String },
        Failure { error: CellDiagnostic },
    }
    let outcome: Outcome =
        serde_json::from_str(&json).map_err(|error| execution(error.to_string()))?;
    let json = match outcome {
        Outcome::Success { json } => json,
        Outcome::Failure { error } => return Err(error),
    };
    if json.len() > max_bytes {
        return Err(CellDiagnostic::limit("result bytes"));
    }
    serde_json::from_str(&json).map_err(|error| execution(error.to_string()))
}
