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

import { CLIENT_SDK_VERSION, type ClientBundle, type ClientDescriptor } from '@maka-agent/plugin-sdk/client';

type Factory = ClientBundle['factory'];
interface Pending {
  readonly id: string;
  factory?: Factory;
  error?: Error;
}
const pendingDocuments = new WeakMap<Document, Map<string, Pending>>();

declare global {
  interface Window { __MakaClientBundle__?: (bundle: ClientBundle) => void }
}

/** Classic scripts have removable URLs and no permanent browser ESM module-map entry. */
export async function loadClientBundle(
  descriptor: ClientDescriptor,
  source: string,
  document: Document,
  signal: AbortSignal,
): Promise<Factory> {
  signal.throwIfAborted();
  if (descriptor.sdkVersion !== CLIENT_SDK_VERSION) throw new Error('Incompatible Client SDK');
  const bytes = new TextEncoder().encode(source);
  if (bytes.length !== descriptor.totalBytes) throw new Error('Client bundle length mismatch');
  const digest = new Uint8Array(await crypto.subtle.digest('SHA-256', bytes));
  const hash = 'sha256-' + Array.from(digest, (byte) => byte.toString(16).padStart(2, '0')).join('');
  if (hash !== descriptor.clientDigest) throw new Error('Client bundle digest mismatch');
  signal.throwIfAborted();
  const window = document.defaultView;
  if (!window) throw new Error('Client document is closed');
  let pending = pendingDocuments.get(document);
  if (!pending) {
    pending = new Map();
    pendingDocuments.set(document, pending);
    const loads = pending;
    window.__MakaClientBundle__ = (bundle) => {
      const script = document.currentScript as HTMLScriptElement | null;
      const request = script ? loads.get(script.src) : undefined;
      if (!request) throw new Error('Client bundle registered outside its loading document');
      if (request.factory || bundle.id !== request.id || typeof bundle.factory !== 'function') {
        request.error = new Error('Invalid or duplicate Client bundle registration');
        throw request.error;
      }
      request.factory = bundle.factory;
    };
  }
  const url = URL.createObjectURL(new Blob([bytes], { type: 'text/javascript' }));
  const script = document.createElement('script');
  const request: Pending = { id: descriptor.extensionId };
  pending.set(url, request);
  script.src = url;
  const scriptError = (event: ErrorEvent) => {
    if (event.filename === url) request.error = new Error(event.message || 'Client bundle threw');
  };
  window.addEventListener('error', scriptError);
  let abort: (() => void) | undefined;
  try {
    await new Promise<void>((resolve, reject) => {
      abort = () => reject(signal.reason);
      const finish = (error?: unknown) => {
        error ? reject(error) : resolve();
      };
      script.onload = () => finish(request.error);
      script.onerror = () => finish(new Error('Client bundle script failed'));
      signal.addEventListener('abort', abort, { once: true });
      document.head.append(script);
      if (signal.aborted) abort();
    });
    signal.throwIfAborted();
    if (!request.factory) throw new Error('Client bundle did not register a factory');
    return request.factory;
  } finally {
    if (abort) signal.removeEventListener('abort', abort);
    window.removeEventListener('error', scriptError);
    script.onload = null;
    script.onerror = null;
    script.remove();
    pending.delete(url);
    URL.revokeObjectURL(url);
  }
}
