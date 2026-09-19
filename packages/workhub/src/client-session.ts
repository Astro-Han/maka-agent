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

import type { ClientContext } from '@maka-agent/plugin-sdk/client';
import type { OperationInput, OperationOutput } from '@maka/runtime-host/protocol';
import type {
  CoordinationSessionServices,
  WorkHubAnswerInput,
  WorkHubAnswerResult,
} from './controller/ports.js';

type Wire<T> = T extends readonly (infer Item)[]
  ? Wire<Item>[]
  : T extends object
    ? { [Key in keyof T]: Wire<T[Key]> }
    : T;
type Outcome<T> = { ok: true; result: T } | { ok: false; error: { code: string; message: string } };
type Answer = Wire<OperationInput<'workhub.coordination.answer'>>;
type Receipt = Wire<OperationOutput<'workhub.coordination.answer'>>;
type ModelInput = Wire<OperationInput<'workhub.coordination.configureModel'>>;
type ModelResult = Wire<OperationOutput<'workhub.coordination.configureModel'>>;

export function coordinationCommands(
  context: Pick<ClientContext, 'remote' | 'hostEpoch' | 'signal'>,
): Pick<CoordinationSessionServices, 'answer' | 'configureModel'> {
  const submit = context.remote.method<Answer, Outcome<Receipt>>('answer');
  const receipt = context.remote.method<Answer, Outcome<Receipt | null>>('answer-receipt');
  const configure = context.remote.method<ModelInput, Outcome<ModelResult>>('configure-model');
  return {
    async answer(_sessionId, input: WorkHubAnswerInput): Promise<WorkHubAnswerResult> {
      context.signal.throwIfAborted();
      const epoch = input.originHostEpoch ?? context.hostEpoch;
      if (!epoch || !context.hostEpoch) throw new Error('WorkHub has no originating Host epoch');
      const { originHostEpoch: _origin, ...request } = input;
      const unknown = (): WorkHubAnswerResult => ({ kind: 'unknown', originHostEpoch: epoch });
      let outcome: Outcome<Receipt>;
      try {
        if (input.originHostEpoch) {
          const proof = await receipt(request);
          if (!proof.ok) {
            if (proof.error.code === 'operation_conflict') throw new DomainError(proof.error);
            return unknown();
          }
          if (proof.result) return { kind: 'admitted', ...proof.result };
          if (epoch !== context.hostEpoch) return { kind: 'not_admitted' };
        }
        context.signal.throwIfAborted();
        outcome = await submit(request);
      } catch (error) {
        if (error instanceof DomainError) throw error;
        // Remote failure cannot prove that an already-dispatched mutation rolled back.
        return unknown();
      }
      if (outcome.ok) return { kind: 'admitted', ...outcome.result };
      if (outcome.error.code === 'outcome_unknown' || input.originHostEpoch) return unknown();
      throw new DomainError(outcome.error);
    },
    async configureModel(_sessionId, input) {
      context.signal.throwIfAborted();
      const outcome = await configure(input);
      if (!outcome.ok) throw new DomainError(outcome.error);
      return outcome.result;
    },
  };
}

class DomainError extends Error {
  constructor(readonly detail: { code: string; message: string }) {
    super(detail.message);
    this.name = 'WorkHubError';
  }
}
