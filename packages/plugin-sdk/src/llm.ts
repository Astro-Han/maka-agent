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

export type ModelChoice = {
  model: { connection_id: string; connection_slug: string; model: string };
  connectionName: string;
  displayName: string;
  thinkingLevels: readonly NonNullable<
    import('./execution.js').ExecutorSettings['thinkingLevel']
  >[];
  defaultThinkingLevel: NonNullable<
    import('./execution.js').ExecutorSettings['thinkingLevel']
  > | null;
  isDefault: boolean;
};
export type ModelChoices = {
  revision: number;
  models: readonly ModelChoice[];
  complete: boolean;
};

export interface ModelGeneration {
  text: string;
  modelId: string;
  finishReason: 'stop' | 'tool-calls' | 'length';
  /** Provider-reported counts; null is unknown, never an invented zero. */
  usage: {
    input_tokens: number | null;
    output_tokens: number | null;
    cache_read_tokens: number | null;
    cache_write_tokens: number | null;
    reasoning_tokens: number | null;
  };
}
export interface Llm {
  /** Uses the authorized source's model binding, Host proxy/OAuth and shared executor.
   * No tools or parent conversation are sent. Result and usage settle before delivery.
   */
  generate(input: {
    prompt: string;
    system?: string;
    /** Defaults to 2048, capped by the configured model limit. */
    maxOutputTokens?: number;
  }): Promise<ModelGeneration>;
}
