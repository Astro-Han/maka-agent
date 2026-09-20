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

import type { PermissionMode } from '@maka/core/permission';
import type { WorkspaceProjection, WorkspaceTarget } from './workspace.js';

export const SKILL_CATALOG_PAGE_MAX_ITEMS = 128;

export const SKILL_CATALOG_PAGE_MAX_BYTES = 48 * 1024;

export const SKILL_CATALOG_PREVIEW_RESULT_MAX_BYTES = 48 * 1024;

export const SKILL_CATALOG_REF_MAX_BYTES = 512;

export const SKILL_CATALOG_DISPLAY_ID_MAX_BYTES = 256;

export const SKILL_CATALOG_NAME_MAX_BYTES = 256;

export const SKILL_CATALOG_DESCRIPTION_MAX_BYTES = 4096;

export const SKILL_CATALOG_CATEGORY_MAX_BYTES = 128;

export const SKILL_CATALOG_STRING_ARRAY_MAX_ITEMS = 64;

export const SKILL_CATALOG_STRING_ARRAY_ITEM_MAX_BYTES = 256;

export type SkillCatalogRevision = `sha256:${string}`;

export type SkillContentSha256 = SkillCatalogRevision;

export type SkillCatalogView = 'governance' | 'bundled' | 'managed_sources';

export type SkillCatalogEntryKind = 'skill' | 'discovery_diagnostic';

export type SkillCatalogSourceType = 'workspace' | 'bundled' | 'managed' | 'unknown';

export type SkillCatalogValidationStatus = 'ok' | 'missing_lock' | 'modified' | 'metadata_error';

export type SkillCatalogManagedUpdateStatus =
  | 'not_managed'
  | 'source_missing'
  | 'up_to_date'
  | 'update_available'
  | 'local_modified'
  | 'metadata_error';

export type SkillCatalogRuntimeStatus = 'enabled' | 'disabled' | 'state_error';

export type SkillCatalogScope = 'project' | 'workspace' | 'user' | 'custom';

export type SkillCatalogDiscoverySource = 'maka' | 'agents' | 'legacy' | 'custom';

export type SkillCatalogContextStatus =
  | 'unknown'
  | 'advertised'
  | 'disabled'
  | 'invalid'
  | 'host_incompatible'
  | 'shadowed'
  | 'budget';

export type SkillCatalogValidationCode =
  | 'missing_lock'
  | 'modified'
  | 'invalid_json'
  | 'id_mismatch'
  | 'unsupported_schema'
  | 'invalid_hash'
  | 'write_failed'
  | 'lock_symlink'
  | 'missing_frontmatter'
  | 'malformed_frontmatter'
  | 'missing_name'
  | 'invalid_name'
  | 'name_too_long'
  | 'missing_description'
  | 'invalid_description'
  | 'description_too_long'
  | 'invalid_allowed_tools'
  | 'invalid_required_tools'
  | 'invalid_required_capabilities'
  | 'invalid_license'
  | 'invalid_compatibility'
  | 'compatibility_too_long'
  | 'invalid_metadata'
  | 'invalid_category'
  | 'unsupported_field'
  | 'body_too_large'
  | 'duplicate_id'
  | 'duplicate_name'
  | 'blocked_path'
  | 'read_failed'
  | 'projection_truncated';

export interface SkillCatalogWorkspaceContext {
  readonly workspace: WorkspaceTarget;
}

export type SkillCatalogInvocableTarget =
  | { readonly kind: 'session'; readonly sessionId: string }
  | {
      readonly kind: 'new_session';
      readonly context: SkillCatalogWorkspaceContext;
      readonly collaborationMode: 'agent' | 'plan';
      readonly permissionMode: PermissionMode;
    };

export interface SkillCatalogInvocableItem {
  readonly ref: string;
  readonly id: string;
  readonly name: string;
  readonly description: string;
}

export interface SkillCatalogGovernanceItem {
  readonly path?: string;
  readonly kind: SkillCatalogEntryKind;
  readonly ref: string;
  readonly id: string;
  readonly name: string;
  readonly description: string;
  readonly declaredTools: readonly string[];
  readonly metadataTruncated: boolean;
  readonly sourceType: SkillCatalogSourceType;
  readonly userModified: boolean;
  readonly validationStatus: SkillCatalogValidationStatus;
  readonly validationCodes: readonly SkillCatalogValidationCode[];
  readonly managedUpdateStatus: SkillCatalogManagedUpdateStatus | null;
  readonly enabled: boolean;
  readonly pinned: boolean;
  readonly runtimeStatus: SkillCatalogRuntimeStatus;
  readonly scope: SkillCatalogScope;
  readonly source: SkillCatalogDiscoverySource;
  readonly contextStatus: SkillCatalogContextStatus;
  readonly contextRank: number | null;
  readonly shadowedBy: string | null;
  readonly needsReview: boolean;
  readonly manageable: boolean;
}

export interface SkillCatalogBundledItem {
  readonly kind: 'bundled';
  readonly id: string;
  readonly name: string;
  readonly description: string;
  readonly category: string;
  readonly declaredTools: readonly string[];
  readonly metadataTruncated: boolean;
  readonly installed: boolean;
}

export interface SkillCatalogManagedSourceItem {
  readonly kind: 'managed_source';
  readonly id: string;
  readonly name: string;
  readonly description: string;
  readonly category: string;
  readonly sourceType: 'local';
  readonly metadataTruncated: boolean;
  readonly installed: boolean;
}

export type SkillCatalogPageItem =
  | SkillCatalogGovernanceItem
  | SkillCatalogBundledItem
  | SkillCatalogManagedSourceItem;

export type SkillCatalogQueryInput =
  | {
      readonly kind: 'start';
      readonly context: SkillCatalogWorkspaceContext;
      readonly view: SkillCatalogView;
    }
  | {
      readonly kind: 'continue';
      readonly context: SkillCatalogWorkspaceContext;
      readonly view: SkillCatalogView;
      readonly revision: SkillCatalogRevision;
      readonly cursor: string;
    };

export type SkillCatalogQueryProjection =
  | {
      readonly kind: 'page';
      readonly view: SkillCatalogView;
      readonly revision: SkillCatalogRevision;
      readonly items: readonly SkillCatalogPageItem[];
      readonly nextCursor: string | null;
    }
  | SkillCatalogRevisionChanged;

export type SkillCatalogQueryResult = SkillCatalogQueryProjection & {
  readonly resolvedWorkspace: WorkspaceProjection;
};

export type SkillCatalogInvocableQueryInput =
  | {
      readonly kind: 'start';
      readonly target: SkillCatalogInvocableTarget;
    }
  | {
      readonly kind: 'continue';
      readonly target: SkillCatalogInvocableTarget;
      readonly revision: SkillCatalogRevision;
      readonly cursor: string;
    };

export type SkillCatalogInvocableQueryResult =
  | {
      readonly kind: 'page';
      readonly revision: SkillCatalogRevision;
      readonly items: readonly SkillCatalogInvocableItem[];
      readonly nextCursor: string | null;
    }
  | {
      readonly kind: 'revision_changed';
      readonly expectedRevision: SkillCatalogRevision;
      readonly actualRevision: SkillCatalogRevision;
    };

export type SkillCatalogMutation =
  | { readonly kind: 'create_starter' }
  | {
      readonly kind: 'install';
      readonly sourceType: 'bundled' | 'managed';
      readonly sourceId: string;
    }
  | SkillCatalogManagedUpdateMutation
  | { readonly kind: 'delete'; readonly ref: string }
  | { readonly kind: 'set_enabled'; readonly ref: string; readonly enabled: boolean }
  | { readonly kind: 'set_pinned'; readonly ref: string; readonly pinned: boolean };

export type SkillCatalogManagedUpdateMutation =
  | {
      readonly kind: 'update_managed';
      readonly ref: string;
      readonly force: false;
      readonly expectedCurrentSha256: null;
      readonly expectedSourceSha256: null;
    }
  | {
      readonly kind: 'update_managed';
      readonly ref: string;
      readonly force: true;
      readonly expectedCurrentSha256: SkillContentSha256;
      readonly expectedSourceSha256: SkillContentSha256;
    };

export interface SkillCatalogMutateInput {
  readonly context: SkillCatalogWorkspaceContext;
  readonly expectedRevision: SkillCatalogRevision;
  readonly mutation: SkillCatalogMutation;
}

export type SkillCatalogMutationRejectedReason =
  | 'not_found'
  | 'already_exists'
  | 'blocked_scope'
  | 'not_managed'
  | 'source_missing'
  | 'source_changed'
  | 'source_invalid'
  | 'local_modified'
  | 'metadata_error'
  | 'needs_review'
  | 'blocked_path'
  | 'state_error';

export type SkillCatalogMutationOutcome =
  | {
      readonly kind: 'committed' | 'unchanged';
      readonly revision: SkillCatalogRevision;
      readonly entry: SkillCatalogGovernanceItem | null;
    }
  | SkillCatalogRevisionConflict
  | { readonly kind: 'rejected'; readonly reason: SkillCatalogMutationRejectedReason };

export type SkillCatalogMutateResult = SkillCatalogMutationOutcome & {
  readonly resolvedWorkspace: WorkspaceProjection;
};

export interface SkillCatalogPreviewUpdateInput {
  readonly context: SkillCatalogWorkspaceContext;
  readonly expectedRevision: SkillCatalogRevision;
  readonly ref: string;
}

export interface SkillCatalogPreviewLineSummary {
  readonly currentLineCount: number;
  readonly sourceLineCount: number;
  readonly changedLineCount: number;
}

export type SkillCatalogPreviewRejectedReason =
  | 'not_found'
  | 'not_managed'
  | 'source_missing'
  | 'source_invalid'
  | 'metadata_error';

export type SkillCatalogPreviewUpdateOutcome =
  | {
      readonly kind: 'preview';
      readonly revision: SkillCatalogRevision;
      readonly currentSnippet: string;
      readonly sourceSnippet: string;
      readonly currentTruncated: boolean;
      readonly sourceTruncated: boolean;
      readonly hasManagedBaseline: boolean;
      readonly summary: SkillCatalogPreviewLineSummary;
      readonly expectedCurrentSha256: SkillContentSha256;
      readonly expectedSourceSha256: SkillContentSha256;
    }
  | SkillCatalogRevisionConflict
  | { readonly kind: 'rejected'; readonly reason: SkillCatalogPreviewRejectedReason };

export type SkillCatalogPreviewUpdateResult = SkillCatalogPreviewUpdateOutcome & {
  readonly resolvedWorkspace: WorkspaceProjection;
};

export interface SkillCatalogRevisionChanged {
  readonly kind: 'revision_changed';
  readonly expectedRevision: SkillCatalogRevision;
  readonly actualRevision: SkillCatalogRevision;
}

export interface SkillCatalogRevisionConflict {
  readonly kind: 'revision_conflict';
  readonly expectedRevision: SkillCatalogRevision;
  readonly actualRevision: SkillCatalogRevision;
}

export interface SkillCatalogResolvePathInput {
  readonly context: SkillCatalogWorkspaceContext;
  readonly ref: string;
  readonly target: 'file' | 'directory';
}

export interface SkillSourceImportInput {
  readonly sourcePath: string;
}

export type SkillSourceImportResult =
  | {
      readonly kind: 'imported';
      readonly source: Pick<
        SkillCatalogManagedSourceItem,
        'id' | 'name' | 'description' | 'category' | 'sourceType'
      >;
    }
  | {
      readonly kind: 'rejected';
      readonly reason: 'invalid_skill' | 'already_exists' | 'blocked_path';
    };

export type SkillCatalogResolvePathResult =
  | { readonly kind: 'resolved'; readonly path: string; readonly target: 'file' | 'directory' }
  | {
      readonly kind: 'rejected';
      readonly reason: 'missing' | 'blocked_path' | 'not_file' | 'not_directory';
    };

export function isSkillCatalogProjectRootLexicallyAbsolute(
  value: string,
  platform: NodeJS.Platform = process.platform,
): boolean {
  if (platform !== 'win32') return value.startsWith('/');
  return /^[A-Za-z]:[\\/]/.test(value) || /^[\\/]{2}[^\\/]+[\\/][^\\/]+(?:[\\/]|$)/.test(value);
}
