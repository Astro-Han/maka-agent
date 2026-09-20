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

import assert from 'node:assert/strict';
import { connect } from 'node:net';
import { once } from 'node:events';
import { parseArgs } from 'node:util';
import {
  connectRemoteRuntimeHost,
  connectRuntimeHostMessageTransport,
} from '../../packages/runtime-host/src/client/connection.ts';
import { FramedTransport } from '../../packages/runtime-host/src/transport/framed-transport.ts';
import {
  INTERACTIVE_RUNTIME_HOST_COMPOSITION_ID,
  RUNTIME_HOST_COMPATIBILITY_EPOCH,
  RUNTIME_HOST_PROTOCOL_VERSION,
} from '../../packages/runtime-host/src/protocol/index.ts';
import { verifySessionWorkflow } from './client-session.mjs';
import { persistentTail } from './client-transcript.mjs';
import { verifyReadWorkflow } from './client-tool.mjs';
import { verifyWriteWorkflow } from './client-write.mjs';
import { verifyCapabilityHost } from './client-capability-host.mjs';
import { verifyManagedApproval } from './client-managed-approval.mjs';
import { verifyForms } from './client-form.mjs';
import { verifyQuestions } from './client-question.mjs';
import { verifyBashWorkflow } from './client-bash.mjs';
import { verifyPatchWorkflow } from './client-patch.mjs';
import { verifyOpenaiOptions } from './client-openai-options.mjs';
import { verifyAnthropicOptions } from './client-anthropic-options.mjs';
import { verifyModelFetch } from './client-model-fetch.mjs';
import { verifyConnectionTest } from './client-connection-test.mjs';
import { verifyAccess } from './client-access.mjs';
import { verifyCapabilityService } from './client-capability-service.mjs';
import { verifyRemoteAccess } from './client-remote-access.mjs';
import { verifyLiveProvider } from './client-live-provider.mjs';
import { verifyMessageQueue } from './client-message-queue.mjs';
import { verifyMessageRecovery } from './client-message-recovery.mjs';
import { verifyMessageSubmit } from './client-message-submit.mjs';
import { verifyMessageInterrupt } from './client-message-interrupt.mjs';
import { verifyOAuthReceipts } from './client-oauth.mjs';
import { verifyOAuthExecution } from './client-oauth-execution.mjs';
import { verifyAutoContext } from './client-auto-context-workflow.mjs';
import { verifyPruning } from './client-pruning-workflow.mjs';
import { verifyContextCompact } from './client-context-compact-workflow.mjs';
import { verifyResume } from './client-resume-workflow.mjs';
import { verifyWorkspaceImage } from './client-workspace-image-workflow.mjs';
import { verifyLargeOutput } from './client-large-output-workflow.mjs';
import { verifyCompatibleChat } from './client-compatible-workflow.mjs';
import { verifyRelayOptions } from './client-relay-workflow.mjs';
import { verifyConsumption } from './client-attachment-workflow.mjs';

const workflows = {
  'resume-workspace': { verify: verifyResume, marker: 'resume' },
  'auto-context-workspace': { verify: verifyAutoContext, marker: 'auto-context' },
  'pruning-workspace': { verify: verifyPruning, marker: 'pruning' },
  'context-compact-workspace': { verify: verifyContextCompact, marker: 'context-compact' },
  'workspace-image-workspace': { verify: verifyWorkspaceImage, marker: 'workspace-image' },
  'large-output-workspace': { verify: verifyLargeOutput, marker: 'large-output' },
  'compatible-workspace': { verify: verifyCompatibleChat, marker: 'compatible-options' },
  'relay-workspace': { verify: verifyRelayOptions, marker: 'relay-options' },
  'attachment-workspace': { verify: verifyConsumption, marker: 'attachment-consumption' },
};

async function main() {
  const { values } = parseArgs({
    options: {
      ...Object.fromEntries(Object.keys(workflows).map((flag) => [flag, { type: 'string' }])),
      socket: { type: 'string' },
      url: { type: 'string' },
      'root-id': { type: 'string' },
      token: { type: 'string' },
      'oauth-receipts': { type: 'string' },
      'oauth-execution-workspace': { type: 'string' },
      'capability-service-workspace': { type: 'string' },
      'capability-host-workspace': { type: 'string' },
      'managed-approval-workspace': { type: 'string' },
      'form-workspace': { type: 'string' },
      'form-disconnect': { type: 'boolean' },
      'question-workspace': { type: 'string' },
      'question-mode': { type: 'string' },
      'session-workspace': { type: 'string' },
      'tool-workspace': { type: 'string' },
      'write-workspace': { type: 'string' },
      'message-queue-workspace': { type: 'string' },
      'message-recovery-workspace': { type: 'string' },
      'message-submit-workspace': { type: 'string' },
      'message-interrupt-workspace': { type: 'string' },
      'message-interrupt-failure-workspace': { type: 'string' },
      'bash-workspace': { type: 'string' },
      'patch-workspace': { type: 'string' },
      'openai-options-workspace': { type: 'string' },
      'anthropic-options-workspace': { type: 'string' },
      'model-fetch-workspace': { type: 'string' },
      'connection-test-workspace': { type: 'string' },
      'access-workspace': { type: 'string' },
      'access-control-directory': { type: 'string' },
      'remote-access-url': { type: 'string' },
      'remote-access-control-directory': { type: 'string' },
      'live-provider-workspace': { type: 'string' },
      reopened: { type: 'boolean' },
      'allow-insecure-remote': { type: 'boolean' },
      help: { type: 'boolean' },
    },
  });
  const selected = Object.entries(workflows).filter(([flag]) => values[flag]);
  assert(selected.length <= 1, 'Supply only one workflow');
  if (values.help) {
    console.log(
      'MAKA_JS_DEPS=/path/to/dependency/repo node tests/fixtures/client.mjs --socket /tmp/host.sock --root-id ID\n  or: --url ws://127.0.0.1:PORT --root-id ID --token TOKEN\n  MAKA_HOST_TOKEN can supply the token without a command-line argument.',
    );
    return;
  }
  assert.equal(
    RUNTIME_HOST_COMPATIBILITY_EPOCH,
    164,
    'Probe baseline changed; review compatibility',
  );
  console.log(
    JSON.stringify({
      check: 'current-source-build',
      protocol: RUNTIME_HOST_PROTOCOL_VERSION,
      compatibilityEpoch: RUNTIME_HOST_COMPATIBILITY_EPOCH,
    }),
  );
  if (values['capability-service-workspace']) {
    await verifyCapabilityService(values['capability-service-workspace']);
    return;
  }
  assert(
    values['root-id'] && Boolean(values.socket) !== Boolean(values.url),
    'Supply --root-id and exactly one of --socket or --url',
  );
  const input = {
    expectedRootId: values['root-id'],
    compositionId: INTERACTIVE_RUNTIME_HOST_COMPOSITION_ID,
    protocol: { min: RUNTIME_HOST_PROTOCOL_VERSION, max: RUNTIME_HOST_PROTOCOL_VERSION },
    handshakeTimeoutMs: 3000,
    livenessIntervalMs: 60000,
  };
  let transport;
  let connection;
  // Also bounds closing: a server that never closes must not hang the probe.
  const deadline = setTimeout(
    () => {
      console.error('Original-client interoperability failed: overall deadline exceeded');
      process.exit(1);
    },
    values['live-provider-workspace']
      ? 320000
      : values['oauth-execution-workspace']
        ? 140000
        : values['message-queue-workspace']
          ? 45000 // Includes 1,030 serial durable commands; each request still has its own deadline.
          : values['large-output-workspace']
            ? 120000
            : values['bash-workspace']
              ? 30000
              : selected.length
                ? 15000
                : 10000,
  );
  try {
    ({ connection, transport } = await openClient(values, input));
    const status = await connection.status(3000);
    if (values['oauth-receipts']) {
      await verifyOAuthReceipts(connection, JSON.parse(values['oauth-receipts']));
    }
    if (values['oauth-execution-workspace']) {
      await verifyOAuthExecution(connection, values['oauth-execution-workspace'], values.reopened);
    }
    assert.equal(status.state, 'ready');
    assert.equal(connection.rootId, values['root-id']);
    assert.equal(status.hostEpoch, connection.hostEpoch);
    assert.equal(status.compositionId, connection.compositionId);
    assert.equal(status.compositionRevision, connection.compositionRevision);
    for (const [flag, { verify, marker }] of selected) {
      await verify(connection, values[flag], values.reopened);
      console.log(`${marker}-${values.reopened ? 'reopened' : 'passed'}`);
    }
    if (values['remote-access-url']) {
      await verifyRemoteAccess(
        connection,
        input,
        values['remote-access-url'],
        values['remote-access-control-directory'],
      );
    }
    if (values['access-workspace']) {
      await verifyAccess(
        connection,
        values['access-workspace'],
        values['access-control-directory'],
        values.reopened,
      );
    }
    if (values['live-provider-workspace']) {
      await verifyLiveProvider(connection, values['live-provider-workspace'], values.reopened);
    }
    if (values['connection-test-workspace']) {
      await verifyConnectionTest(
        connection,
        values['connection-test-workspace'],
        values.reopened,
        async () => (await openClient(values, input)).connection,
      );
    }
    if (values['model-fetch-workspace']) {
      await verifyModelFetch(
        connection,
        values['model-fetch-workspace'],
        values.reopened,
        async () => (await openClient(values, input)).connection,
      );
    }
    if (values['openai-options-workspace']) {
      await verifyOpenaiOptions(connection, values['openai-options-workspace'], values.reopened);
    }
    if (values['anthropic-options-workspace']) {
      await verifyAnthropicOptions(
        connection,
        values['anthropic-options-workspace'],
        values.reopened,
      );
    }
    if (values['patch-workspace']) {
      await verifyPatchWorkflow(connection, values['patch-workspace'], values.reopened);
    }
    if (values['bash-workspace']) {
      await verifyBashWorkflow(
        connection,
        values['bash-workspace'],
        values.reopened,
        async () => (await openClient(values, input)).connection,
      );
    }
    if (values['write-workspace']) {
      await verifyWriteWorkflow(connection, values['write-workspace'], values.reopened);
    }
    if (values['message-queue-workspace']) {
      await verifyMessageQueue(
        connection,
        values['message-queue-workspace'],
        values.reopened,
        async () => (await openClient(values, input)).connection,
      );
    }
    if (values['message-recovery-workspace']) {
      await verifyMessageRecovery(connection, values.reopened);
    }
    if (values['capability-host-workspace']) {
      await verifyCapabilityHost(
        connection,
        values['capability-host-workspace'],
        values.reopened,
        async () => (await openClient(values, input)).connection,
      );
    }
    if (values['managed-approval-workspace']) {
      await verifyManagedApproval(
        connection,
        values['managed-approval-workspace'],
        values.reopened,
        async () => (await openClient(values, input)).connection,
      );
    }
    if (values['question-workspace']) {
      await verifyQuestions(
        connection,
        values['question-workspace'],
        values.reopened,
        values['question-mode'],
      );
    }
    if (values['form-workspace']) {
      await verifyForms(
        connection,
        values['form-workspace'],
        values.reopened,
        () => openClient(values, input),
        values['form-disconnect'],
      );
    }
    if (values['tool-workspace']) {
      await verifyReadWorkflow(connection, values['tool-workspace'], values.reopened);
    }
    if (values['session-workspace']) {
      // Legacy Turn API remains separately covered.
      await verifySessionWorkflow(
        connection,
        values['session-workspace'],
        values.reopened,
        async () => (await openClient(values, input)).connection,
      );
      await persistentTail(connection, values['session-workspace'], values.reopened);
    }
    if (values['message-submit-workspace']) {
      await verifyMessageSubmit(
        connection,
        values['message-submit-workspace'],
        values.reopened,
        async () => (await openClient(values, input)).connection,
      );
    }
    if (values['message-interrupt-workspace']) {
      await verifyMessageInterrupt(
        connection,
        values['message-interrupt-workspace'],
        values.reopened,
        async () => (await openClient(values, input)).connection,
      );
    }
    if (values['message-interrupt-failure-workspace']) {
      await verifyMessageInterrupt(
        connection,
        values['message-interrupt-failure-workspace'],
        false,
        async () => (await openClient(values, input)).connection,
        true,
      );
    }
    await connection.close();
    await connection.closed;
    console.log(
      JSON.stringify({
        check: 'original-client-interoperability',
        result: 'passed',
        rootId: connection.rootId,
        selectedProtocol: connection.selectedProtocol,
        status,
        closed: true,
      }),
    );
  } finally {
    transport?.abort();
    if (connection) await connection.close();
    clearTimeout(deadline);
  }
}

async function openClient(values, input) {
  let transport;
  try {
    let result;
    if (values.socket) {
      assert(!values.token, '--token is only used with --url');
      const socket = connect(values.socket);
      transport = new FramedTransport(socket);
      await once(socket, 'connect');
      result = await connectRuntimeHostMessageTransport({ ...input, transport });
    } else {
      const credential = values.token ?? process.env.MAKA_HOST_TOKEN;
      assert(credential, 'WebSocket connections require --token or MAKA_HOST_TOKEN');
      result = await connectRemoteRuntimeHost({
        ...input,
        url: values.url,
        credential,
        connectTimeoutMs: 3000,
        allowInsecureRemote: values['allow-insecure-remote'],
      });
    }
    assert.equal(result.kind, 'connected', JSON.stringify(result));
    return { connection: result.connection, transport };
  } catch (error) {
    transport?.abort();
    throw error;
  }
}

main().catch((error) => {
  console.error(`Original-client probe failed: ${error.message}`);
  process.exitCode = 1;
});
