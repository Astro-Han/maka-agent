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
import { once } from 'node:events';
import { createServer, request as forward } from 'node:http';

// Exercise the unchanged client's atomic proxy mutation and actual HTTP proxy
// routing while the existing provider scenario retains all its wire assertions.
export async function withNetworkProxy(connection, baseUrl, run) {
  const request = (op, input) => connection.request(op, input, 3000);
  const initial = await request('runtime.policy.query', {});
  const locator = { scope: 'network_proxy', kind: 'password' };
  const prior = await request('credential.vault.query', { locator });
  assert.equal(prior.status.configured, false);
  let count = 0;
  let failure;
  const server = createServer((incoming, response) => {
    try {
      const target = new URL(incoming.url);
      assert.equal(target.origin, new URL(baseUrl).origin);
      assert.equal(incoming.headers['proxy-authorization'], 'Basic dXNlcjpmaXh0dXJlLXByb3h5');
      count++;
      if (target.href === new URL(`${baseUrl}/__proxy_probe`).href) {
        response.writeHead(200, { 'Content-Length': 0 });
        response.end();
        return;
      }
      const headers = { ...incoming.headers };
      delete headers['proxy-authorization'];
      const upstream = forward(target, { method: incoming.method, headers }, (result) => {
        response.writeHead(result.statusCode, result.headers);
        result.pipe(response);
      });
      upstream.on('error', (error) => {
        failure = error;
        response.destroy(error);
      });
      incoming.on('aborted', () => upstream.destroy());
      response.on('close', () => upstream.destroy());
      incoming.pipe(upstream);
    } catch (error) {
      failure = error;
      response.destroy(error);
    }
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  let credential;
  try {
    const disabled = await request('network-proxy.test', { url: `${baseUrl}/__proxy_probe` });
    assert.equal(disabled.ok, false);
    assert.equal(disabled.error, 'Proxy disabled');
    const policy = {
      ...initial.policy.networkProxy,
      enabled: true,
      protocol: 'http',
      host: '127.0.0.1',
      port: server.address().port,
      authEnabled: true,
      username: 'user',
      bypassList: [],
      autoBypassDomains: [],
    };
    const input = {
      expectedPolicyRevision: initial.revision,
      expectedCredential: null,
      networkProxy: policy,
      credential: { kind: 'replace', secret: 'fixture-proxy' },
    };
    const changed = await request('runtime.policy.network-proxy.update', input);
    credential = changed;
    assert.equal(changed.kind, 'committed');
    assert.equal(changed.credentialStatus.configured, true);
    const basis = {
      locator,
      credentialId: changed.credentialStatus.credentialId,
      revision: changed.credentialStatus.revision,
    };
    assert.equal(
      (await request('runtime.policy.network-proxy.update', input)).kind,
      'revision_conflict',
    );
    assert.equal(
      (
        await request('runtime.policy.network-proxy.update', {
          ...input,
          expectedPolicyRevision: changed.revision,
        })
      ).kind,
      'credential_stale',
    );
    assert.deepEqual(
      await request('runtime.policy.network-proxy.update', {
        ...input,
        expectedPolicyRevision: changed.revision,
        expectedCredential: basis,
      }),
      changed,
    );
    const configured = await request('runtime.policy.query', {});
    const probe = await request('network-proxy.test', {
      url: `${baseUrl}/__proxy_probe`,
      timeoutMs: 500,
    });
    assert.equal(probe.ok, true);
    assert.equal(probe.status, 200);
    const countBeforeDraft = count;
    const draftProbe = await request('network-proxy.test', {
      networkProxy: { ...policy, bypassList: ['*'], autoBypassDomains: ['*'] },
      url: `${baseUrl}/__proxy_probe`,
      timeoutMs: 500,
    });
    assert.equal(draftProbe.ok, true);
    assert.equal(
      count,
      countBeforeDraft + 1,
      'diagnostic must test the proxy even for bypassed destinations',
    );
    const changedAccount = await request('network-proxy.test', {
      networkProxy: { ...policy, username: 'different-account' },
      url: `${baseUrl}/__proxy_probe`,
      timeoutMs: 500,
    });
    assert.equal(changedAccount.ok, false);
    assert.equal(
      count,
      countBeforeDraft + 1,
      'a draft must not forward saved credentials to another target',
    );
    assert.deepEqual(
      await request('runtime.policy.query', {}),
      configured,
      'diagnostics do not persist drafts',
    );
    await run();
    if (failure) throw failure;
    assert(count > 0, 'model probes must reach the proxy, not only the origin fixture');
    assert.deepEqual((await request('runtime.policy.query', {})).policy.networkProxy, policy);
  } finally {
    try {
      const current = await request('runtime.policy.query', {});
      if (credential?.kind === 'committed') {
        const deleted = await request('runtime.policy.network-proxy.update', {
          expectedPolicyRevision: current.revision,
          networkProxy: initial.policy.networkProxy,
          credential: { kind: 'delete' },
          expectedCredential: {
            locator,
            credentialId: credential.credentialStatus.credentialId,
            revision: credential.credentialStatus.revision,
          },
        });
        assert.equal(deleted.kind, 'committed');
      }
    } finally {
      server.closeAllConnections();
      await new Promise((resolve) => server.close(resolve));
    }
  }
}
