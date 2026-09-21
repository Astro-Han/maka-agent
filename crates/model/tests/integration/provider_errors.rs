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

use deno_core::JsRuntime;

#[tokio::test(flavor = "current_thread")]
async fn provider_evidence_is_bounded_and_output_survives_normalization() {
    let mut runtime = JsRuntime::new(Default::default());
    runtime
        .execute_script(
            "network-fetch",
            include_str!("../../../js-runtime/trusted/network-fetch.js").replace("export ", ""),
        )
        .unwrap();
    runtime
        .execute_script(
            "provider-errors",
            include_str!("../../../js-runtime/trusted/provider-errors.js")
                .replace(
                    "import { isTransportFailure } from './network-fetch.js';",
                    "",
                )
                .replace("export ", ""),
        )
        .unwrap();
    let promise = runtime.execute_script("overflow-contract", r#"
      (async () => {
        const assert = (ok, message) => { if (!ok) throw Error(message); };
        const overflow = { code: 'context_length_exceeded', message: 'rejected' };
        const api = (fields) => Object.assign(new Error('provider'), {name:'AI_APICallError'}, fields);
        for (const value of [overflow, {error:overflow}, {data:{error:overflow}},
          api({responseBody:JSON.stringify({error:overflow})}),
          api({data:{type:'response.failed',response:{error:overflow}}}),
          {type:'request_too_large'}, {code:'model_context_window_exceeded'}]) {
          assert(isContextOverflow(value, 'openai_chat'), 'structured overflow lost');
        }
        for (const value of [{statusCode:413}, {message:'context_length_exceeded'},
          {code:'rate_limit_error',message:'context overflow'},
          {data:{prompt:{code:'context_length_exceeded'}}},
          api({responseBody:' '.repeat(65537)+JSON.stringify({error:overflow})}),
          Object.assign(new Error('abort'),overflow,{name:'AbortError'}),
          Object.assign(new Error('parse'),{name:'AI_TypeValidationError',data:overflow}),
          api({responseBody:'not JSON'})]) {
          assert(!isContextOverflow(value, 'openai_chat'), 'weak error became overflow');
        }
        const anthropic = {statusCode:400,data:{error:{type:'invalid_request_error',message:'prompt is too long: 200001 tokens > 200000 maximum'}}};
        assert(isContextOverflow(anthropic,'anthropic'),'Anthropic relation lost');
        assert(!isContextOverflow(anthropic,'openai_chat'),'Anthropic rule escaped route');
        for (const message of ['prompt is too long', 'prompt is too long: 1 tokens > 2 maximum',
          'quota: prompt is too long: 3 tokens > 2 maximum',
          'prompt is too long: 3 tokens > 2 maximum; output limit',
          'prompt is too long: 9999999999999999 tokens > 2 maximum']) {
          anthropic.data.error.message = message;
          assert(!isContextOverflow(anthropic,'anthropic'),'invalid token relation accepted');
        }
        const errorPart = {type:'error',error:overflow};
        const open = (...parts) => async () => ({stream:(async function*(){yield* parts;})()});
        for (const part of [{type:'text-start',id:'text'}, {type:'reasoning-start',id:'reason'},
          {type:'tool-input-start',id:'tool',providerExecuted:true},
          {type:'response-metadata',id:'r',providerMetadata:{anthropic:{signature:'sig'}}},
          {type:'unknown-activity'}]) {
          const emitted = [];
          await forwardProviderStream(open(part,errorPart), () => undefined, async value => emitted.push(value), 'openai_chat');
          assert(emitted[0].error.observedOutput === true, 'normalization hid output');
        }
        const emitted = [];
        await forwardProviderStream(open({type:'stream-start'},{type:'raw',rawValue:{}},
          {type:'response-metadata',id:'request',timestamp:'time'},errorPart),
          part => part, async value => emitted.push(value), 'openai_chat');
        assert(emitted.at(-1).error.observedOutput === false,'handshake became output');
        for (const start of [async () => {throw api({data:{error:overflow}})},
          async () => ({stream:(async function*(){throw api({data:{error:overflow}})})()})]) {
          const errors = [];
          await forwardProviderStream(start,part=>part,async part=>errors.push(part),'openai_chat');
          assert(errors[0].error.kind === 'context_overflow','throw classification lost');
        }
        const local = api({data:{error:overflow}});
        let escaped;
        try { await forwardProviderStream(open({type:'text-start'}),part=>part,async()=>{throw local},'openai_chat'); }
        catch (error) { escaped = error; }
        assert(escaped === local,'local emit failure was classified');
      })()
    "#).unwrap();
    let resolving = runtime.resolve(promise);
    runtime
        .with_event_loop_promise(resolving, Default::default())
        .await
        .unwrap();
}
