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

import type { ArtifactBinaryReadResult } from '@maka/core/artifacts';
import type { UiLocale } from '@maka/core/ui-locale';
import type { ComposerAttachmentService } from '@maka/ui/use-composer-attachments';
import type { AttachmentRef } from '@maka/core/events';
import type { CoordinationSessionAdapter } from '@maka/workhub/controller';
import type { WorkHubControlBridge } from '../../../shared/workhub-control.js';
import type { WorkHubPresentationBridge } from '../../../shared/workhub-presentation.js';

export type { WorkHubTranscriptSnapshot } from '@maka/workhub/controller';

/** Desktop adapters; not the plugin SDK. */
export interface WorkHubServices extends CoordinationSessionAdapter {
  readonly inspector: import('../../application/contracts/session-inspector/service.js').SessionInspectorService;
  readonly surface: 'main' | 'workhub';
  readonly initialLocale: UiLocale;
  subscribeAppearance(handler: (locale: UiLocale) => void): () => void;
  readonly presentation: WorkHubPresentationBridge;
  readonly control: WorkHubControlBridge;
  bindBrowserSession(sessionId: string | null): void;
  readonly attachments: ComposerAttachmentService;
  readAttachmentBytes(sessionId: string, artifactId: string): Promise<ArtifactBinaryReadResult>;
  prepareAttachments(sessionId: string, items: Array<{ approvalId: string; name: string; mimeType?: string } | { file: File }>): Promise<AttachmentRef[]>;
}
