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

/** @param {import('../../../../packages/plugin-sdk/src/host.js').HostContext} ctx */
export default async function (ctx) {
  /** @type {import('../../../../packages/plugin-sdk/src/host.js').HttpResponse | undefined} */
  let previous;
  await ctx.executors.register(
    { name: 'example.http', displayName: 'HTTP acceptance' },
    async (request, call) => {
      const base = 'http://maka-http.invalid';
      if (request.content.text === 'denied') {
        try {
          await call.http.request({ url: base });
          throw new Error('Ask invocation gained raw HTTP');
        } catch (error) {
          if (error.code !== 'revoked') throw error;
        }
        return { status: 'completed', text: 'Denied correctly' };
      }
      if (previous) {
        try {
          await previous.next();
          throw new Error('old HTTP invocation retained authority');
        } catch (error) {
          if (error.code !== 'revoked') throw error;
        }
        await previous.close();
      }
      const upload = await call.http.request({
        url: `${base}/upload`,
        method: 'POST',
        body: new Uint8Array(1024 * 1024).fill(255),
      });
      if (upload.status !== 204) throw new Error('maximum HTTP upload failed');
      await upload.close();
      try {
        await call.http.request({
          url: `${base}/unexpected`,
          method: 'POST',
          body: new Uint8Array(1024 * 1024 + 1).fill(255),
        });
        throw new Error('oversized HTTP body was sent');
      } catch (error) {
        if (!error.message.includes('request limit')) throw error;
      }
      const response = await call.http.request({
        url: `${base}/stream`,
        method: 'POST',
        body: 'request 測試🦀',
      });
      previous = response;
      if (
        response.status !== 200 ||
        response.headers.filter(([name]) => name === 'x-repeat').length !== 2
      )
        throw new Error('HTTP metadata lost');
      const decoder = new TextDecoder('utf-8', { fatal: true });
      let content = '';
      for (;;) {
        const chunk = await response.next();
        if (chunk === null) break;
        if (chunk.length > 16384) throw new Error('unbounded HTTP chunk');
        content += decoder.decode(chunk, { stream: true });
      }
      content += decoder.decode();
      if (content !== '測試🦀'.repeat(5000)) throw new Error('HTTP body damaged');
      const truncated = await call.http.request({ url: `${base}/truncated` });
      try {
        while ((await truncated.next()) !== null) {
          /* Drain to the framing error. */
        }
        throw new Error('truncated HTTP body silently accepted');
      } catch (error) {
        if (!error.message.includes('body')) throw error;
      } finally {
        await truncated.close();
      }
      const redirect = await call.http.request({
        url: `${base}/redirect`,
        method: 'POST',
        body: 'once',
      });
      if (redirect.status !== 307) throw new Error('HTTP silently replayed request');
      await redirect.close();
      const blocked = await call.http.request({ url: `${base}/blocked` });
      if (request.content.text === 'retire') {
        await ctx.storage.batch([
          { key: 'waiting', expectedRevision: null, data: { kind: 'present', value: true } },
        ]);
        // Retirement must reject a pending read instead of reporting a clean EOF.
        await blocked.next();
        try {
          await blocked.next();
          throw new Error('retirement reported successful end of body');
        } catch (error) {
          if (!/retired|invocation|closed/.test(error.message)) throw error;
        }
      }
      // Leave a response unclosed: invocation settlement must cancel its I/O.
      return { status: 'completed', text: 'HTTP verified' };
    },
  );
}
