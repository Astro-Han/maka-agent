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

import { createOpenAI } from '@ai-sdk/openai';
import { createAnthropic, anthropic } from '@ai-sdk/anthropic';
import { createOpenAICompatible } from '@ai-sdk/openai-compatible';
import { compatibleFetch, compatibleEvents } from './compatible-transport.js';
import { forwardProviderStream } from './provider-errors.js';
import { boundedFetch, ProviderResponseLimitError } from './provider-fetch.js';
import { networkFetch } from './network-fetch.js';

// SDK metadata distinguishes server execution from provider-defined local tools.
// Derive this from the installed SDK, not a parallel list of vendor tool names.
const providerExecutedTools = new Set(
  Object.values(anthropic.tools)
    .map((factory) => factory({}))
    .filter((tool) => tool.isProviderExecuted === true)
    .map((tool) => tool.id),
);

// SDKs own one model request only. Rust owns turns, history and effect execution.
export async function stream(request, emit, signal, requestId) {
  for (const tool of request.tools ?? []) {
    if (tool.type === 'provider' && !providerExecutedTools.has(tool.id)) {
      throw new Error(`Unsupported provider-executed tool: ${tool.id}`);
    }
  }
  const { kind, model, baseUrl, apiKey, requestHeaders, headers, bodyOverlay } = request.provider;
  const fetch = networkFetch(requestId);
  const scopedFetch = (input, init) => boundedFetch(input, init, fetch);
  const settings = {
    baseURL: baseUrl,
    apiKey: apiKey ?? '',
    fetch: scopedFetch,
  };
  const compatible = kind?.openai_compatible;
  if (!compatible && kind !== 'anthropic' && kind !== 'openai_chat') {
    throw new Error('Unsupported AI SDK protocol');
  }
  const overlayKeys = Object.keys(bodyOverlay ?? {});
  if (
    requestHeaders !== undefined ||
    Object.keys(headers ?? {}).length > 0 ||
    overlayKeys.length > 0
  ) {
    settings.fetch = async (input, init) => {
      const generated = new Request(input, init);
      if (requestHeaders !== undefined) {
        generated.headers.delete('authorization');
        generated.headers.delete('x-api-key');
        for (const [name, value] of Object.entries(requestHeaders)) {
          const current = generated.headers.get(name);
          if (current !== null && current !== value) {
            throw new Error(`Authentication header conflicts with a protocol header: ${name}`);
          }
          generated.headers.set(name, value);
        }
      }
      for (const [name, value] of Object.entries(headers ?? {})) {
        const current = generated.headers.get(name);
        if (current !== null && current !== value) {
          throw new Error(`Custom request header conflicts with a generated header: ${name}`);
        }
        generated.headers.set(name, value);
      }
      const contentType = generated.headers.get('content-type');
      if (
        overlayKeys.length > 0 &&
        generated.body !== null &&
        generated.method !== 'GET' &&
        generated.method !== 'HEAD' &&
        (contentType === null ||
          /(^|\s|;)application\/(?:[\w.+-]+\+)?json(?:\s*;|$)/i.test(contentType))
      ) {
        const invalidBody = 'Extra request body can only be applied to a JSON object request';
        const body = await generated
          .clone()
          .json()
          .catch(() => {
            throw new Error(invalidBody);
          });
        if (body === null || typeof body !== 'object' || Array.isArray(body)) {
          throw new Error(invalidBody);
        }
        for (const key of overlayKeys) {
          if (Object.hasOwn(body, key)) {
            throw new Error(`Extra request body conflicts with a generated field: ${key}`);
          }
        }
        generated.headers.delete('content-length');
        return scopedFetch(
          new Request(generated, {
            body: JSON.stringify({ ...body, ...bodyOverlay }),
          }),
        );
      }
      return scopedFetch(generated);
    };
  }
  if (compatible) settings.fetch = compatibleFetch(settings.fetch, requestId);
  const instance = compatible
    ? createOpenAICompatible({ ...settings, name: compatible.name, includeUsage: true })(model)
    : kind === 'anthropic'
      ? createAnthropic({
          ...settings,
          headers: {
            'anthropic-beta':
              'interleaved-thinking-2025-05-14,fine-grained-tool-streaming-2025-05-14',
          },
        })(model)
      : createOpenAI(settings).chat(model);
  const open = async () => {
    const result = await instance.doStream({
      abortSignal: signal,
      prompt: request.prompt,
      tools: request.tools,
      providerOptions: request.providerOptions,
      maxOutputTokens: request.maxOutputTokens,
      includeRawChunks: !!compatible,
      toolChoice: request.tools?.length ? { type: 'auto' } : undefined,
    });
    return result;
  };
  const normalize = compatible ? compatibleEvents() : (part) => part;
  try {
    await forwardProviderStream(open, normalize, emit, kind);
  } catch (error) {
    // SDK transport wrappers retain causes but their display omits the local
    // limit. Preserve only our actual error class, never provider-controlled text.
    let cause = error;
    for (let depth = 0; cause instanceof Error && depth < 8; depth++, cause = cause.cause) {
      if (cause instanceof ProviderResponseLimitError) throw cause;
    }
    throw error;
  }
}
