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

import { RemoteError, type ClientContext } from '@maka-agent/plugin-sdk/client';
import type {
  MessageReceipt,
  Configured,
  ExecutionTarget,
  ExecutionReceipt,
  ExecutionObservation,
} from '@maka-agent/plugin-sdk/host';
import type { OperationInput } from '@maka/runtime-host/protocol';
import type { HostAttachments } from './slots.js';
import type { CoordinationCommands, WorkHubAnswerInput } from './controller/ports.js';

type Wire<T> = T extends readonly (infer Item)[]
  ? Wire<Item>[]
  : T extends object
    ? { [Key in keyof T]: Wire<T[Key]> }
    : T;
type Answer = Wire<WorkHubAnswerInput>;
type ModelInput = {
  sessionId: string;
  expectedRevision: number;
  target: Extract<ExecutionTarget, { kind: 'model' }>;
};
type Enqueue = Pick<
  Wire<OperationInput<'turn.message.submit'>>,
  'messageId' | 'content' | 'placement'
> & {
  expectedTurnId: string;
};

export function coordinationCommands(
  context: Pick<ClientContext, 'remote' | 'signal'>,
  toHost: HostAttachments,
  hostSessionId: string | undefined,
): CoordinationCommands {
  const submit = context.remote.method<Answer, Wire<ExecutionReceipt>>('answer', hostSessionId);
  const receipt = context.remote.method<Answer, Wire<ExecutionObservation> | null>(
    'answer-receipt',
    hostSessionId,
  );
  const requireSession = () => {
    context.signal.throwIfAborted();
    if (!hostSessionId) throw new Error('WorkHub Session is not resolved');
    return hostSessionId;
  };
  const request = (sessionId: string, input: WorkHubAnswerInput): Answer => ({
    ...input,
    ...(input.attachments ? { attachments: toHost(sessionId, input.attachments) } : {}),
  });
  return {
    async enqueueMessage(sessionId, messageId, text, attachments, placement, expectedTurnId) {
      requireSession();
      const content = { text, attachments: toHost(sessionId, attachments) };
      try {
        await context.remote.method<Enqueue, Wire<MessageReceipt>>(
          'enqueue',
          hostSessionId,
        )({
          expectedTurnId,
          messageId,
          content,
          placement,
        });
        return 'admitted';
      } catch (error) {
        return rejected(error) ? 'rejected' : 'unknown';
      }
    },
    async answer(sessionId, input) {
      requireSession();
      const content = request(sessionId, input);
      try {
        const proof = await receipt(content);
        if (proof) return { kind: 'admitted', receipt: proof.receipt, progress: proof.progress };
        context.signal.throwIfAborted();
        return { kind: 'admitted', receipt: await submit(content) };
      } catch (error) {
        if (rejected(error)) throw error;
        // A lost reply is not permission to choose a new operation identity.
        return { kind: 'unknown' };
      }
    },
    async cancelAnswer(sessionId, input) {
      requireSession();
      await context.remote.method<Answer, Wire<ExecutionObservation>>(
        'answer-cancel',
        hostSessionId,
      )(request(sessionId, input));
    },
    async configureModel(_sessionId, input) {
      const sessionId = requireSession();
      return context.remote.method<ModelInput, Wire<Configured>>(
        'configure-model',
        hostSessionId,
      )({
        ...input,
        sessionId,
      });
    },
  };
}
function rejected(error: unknown): boolean {
  return error instanceof RemoteError && error.code === 'invalid_request';
}
