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

use crate::Operation;

impl Operation {
    /// Explicit remote-owner allowlist, matching REMOTE_OWNER_OPERATION_GRANTS.
    /// New operation variants remain denied until deliberately added here.
    pub const fn allows_remote_owner(self) -> bool {
        matches!(
            self,
            Self::AccessCredentialFinalize
                | Self::AgentGraphEpochsQuery
                | Self::AgentGraphOperatorQuery
                | Self::AgentGraphQuery
                | Self::AgentGraphStop
                | Self::ArtifactDelete
                | Self::ArtifactIngest
                | Self::ArtifactQuery
                | Self::ClientCapabilityReplace
                | Self::ClientCapabilityUnregister
                | Self::ConfigurationCredentialsExport
                | Self::CollaborationAccessQuery
                | Self::CollaborationGrantRevoke
                | Self::CollaborationInvitationPrepare
                | Self::CollaborationPrincipalRevoke
                | Self::CollaborationPrincipalRename
                | Self::CollaborationTurnRequestDecide
                | Self::CollaborationTurnRequestQuery
                | Self::ConnectionCatalogCreate
                | Self::ConnectionCatalogQuery
                | Self::ConnectionCatalogRemove
                | Self::ConnectionCatalogSetDefaultTarget
                | Self::ConnectionCatalogUpdate
                | Self::ConnectionModelsFetch
                | Self::ConnectionOnboardingSave
                | Self::ConnectionOnboardingVerify
                | Self::ConnectionRequestHeadersQuery
                | Self::ConnectionRequestHeadersReplace
                | Self::ConnectionTestRun
                | Self::ContextCompact
                | Self::ContextDiagnosticsQuery
                | Self::CredentialVaultDelete
                | Self::CredentialVaultQuery
                | Self::CredentialVaultSet
                | Self::DailyReviewMutate
                | Self::DailyReviewQuery
                | Self::DeepResearchQuery
                | Self::ExecutionInspectQuery
                | Self::ExternalSessionCatalogQuery
                | Self::ExternalSessionImport
                | Self::ExternalSessionSourceQuery
                | Self::GoalArm
                | Self::GoalControl
                | Self::GoalQuery
                | Self::HostDiagnosticsQuery
                | Self::HostResourcesQuery
                | Self::HostStatus
                | Self::InteractionAnswer
                | Self::InteractionQuery
                | Self::MemoryMutate
                | Self::MemoryQuery
                | Self::NetworkProxyTest
                | Self::OauthEnrollmentQuery
                | Self::OauthLoginCancel
                | Self::OauthLoginQuery
                | Self::OauthLoginStart
                | Self::PlanControl
                | Self::PlanQuery
                | Self::PlanTurnStart
                | Self::PricingMutate
                | Self::PricingQuery
                | Self::ProjectCatalogMutate
                | Self::ProjectCatalogQuery
                | Self::QueueEntriesReorder
                | Self::QueueEntryPromote
                | Self::QueueEntryRetract
                | Self::QueueEntryUpdate
                | Self::QueueRetract
                | Self::RuntimePolicyMutate
                | Self::RuntimePolicyNetworkProxyUpdate
                | Self::RuntimePolicyQuery
                | Self::RuntimeResourceControllerAcquire
                | Self::RuntimeResourceControllerControl
                | Self::RuntimeResourceControllerRelease
                | Self::RuntimeResourceQuery
                | Self::RuntimeResourceStart
                | Self::RuntimeResourceStop
                | Self::ScheduledTaskMutate
                | Self::ScheduledTaskQuery
                | Self::SessionBranchCreate
                | Self::SessionCatalogQuery
                | Self::SessionConfigurationUpdate
                | Self::SessionCreate
                | Self::SessionExecutionBoundaryQuery
                | Self::SessionLifecycleSet
                | Self::SessionSharedQuery
                | Self::SessionMetadataUpdate
                | Self::SessionReadMarkerSet
                | Self::SessionRecapGenerate
                | Self::SessionRemove
                | Self::SessionRemovePreview
                | Self::SessionRevisionAbandon
                | Self::SessionRevisionCreate
                | Self::SessionTranscriptPage
                | Self::SessionTranscriptOverlayRelease
                | Self::SessionTurnLandmarksQuery
                | Self::SessionTurnsQuery
                | Self::SessionWorkspaceRelocate
                | Self::SkillCatalogInvocableQuery
                | Self::SkillCatalogMutate
                | Self::SkillCatalogPreviewUpdate
                | Self::SkillCatalogQuery
                | Self::SubscriptionClose
                | Self::SubscriptionOpen
                | Self::SubscriptionPtyInterestSet
                | Self::SessionTodoQuery
                | Self::TurnInterrupt
                | Self::TurnMessageExecutionQuery
                | Self::TurnMessageQuery
                | Self::TurnMessageSubmit
                | Self::TurnQuery
                | Self::TurnRegenerate
                | Self::TurnResumeQuery
                | Self::TurnResumeStart
                | Self::TurnStart
                | Self::TurnStop
                | Self::UsageQuery
                | Self::WebSearchExecute
                | Self::WorkhubCoordinationAnswer
                | Self::WorkhubCoordinationActFromTurn
                | Self::WorkhubCoordinationCandidates
                | Self::WorkhubCoordinationConfigureModel
                | Self::WorkhubCoordinationQuery
                | Self::WorkhubCoordinationResolve
                | Self::WorkhubCoordinationSelectAndDelegate
        )
    }
}
