// 设置 · 子 Agent — the approved model routes the main agent may delegate to.
//
// Two levels, one container, one back affordance, and nothing modal:
//
//   list ── editor(new | existing)
//
// A modal exists to interrupt the current task for something short and
// immediately decidable; naming a capability, writing the guidance the main
// agent selects on, and picking a connection/model/thinking route is none of
// those. The providers panel next door already answers this same list→detail
// shape with a route level, so this page follows it rather than inventing a
// second answer — including its focus controller, which is shared.
//
// Every element here is a settings-kit part (`SettingsSection`, `SettingsRow`,
// `SettingsField`, `SettingsActions`) or an Astryx primitive. The page owns no
// CSS of its own; `.settingsRouteLevel` is the shared route-level focus reset.
import { useEffect, useId, useMemo, useRef, useState } from 'react';
import { Banner, HStack, VStack } from '@astryxdesign/core';
import {
  connectionEnabledModelIds,
  isSafeSubagentPresetId,
  MAX_SUBAGENT_PRESETS,
  SUBAGENT_PRESET_DESCRIPTION_MAX_CHARS,
  SUBAGENT_PRESET_NAME_MAX_CHARS,
  thinkingVariantsForModel,
  type AppSettings,
  type LlmConnection,
  type SubagentPreset,
  type SubagentProfile,
  type ThinkingLevel,
  type UpdateAppSettingsResult,
} from '@maka/core';
import {
  Badge,
  Button,
  EmptyState,
  IconButton,
  Selector,
  Switch,
  TextArea,
  TextInput,
  type SelectorOptionData,
  useToast,
  useUiLocale,
} from '@maka/ui';
import { ChevronRight } from '@maka/ui/icons';
import { getSubagentSettingsCopy } from '../locales/settings-subagents-copy.js';
import { settingsActionErrorMessage } from './settings-error-copy.js';
import { useSettingsRouteFocus } from './settings-route-focus.js';
import { SettingsRouteHeader } from './settings-route-header.js';
import {
  SettingsActions,
  SettingsField,
  SettingsPage,
  SettingsRow,
  SettingsSection,
} from './settings-section.js';
import { SettingRow } from './settings-rows.js';
import {
  nextSubagentDraftForName,
  resolveSubagentRoute,
  subagentPresetAvailability,
  type SubagentPageRoute,
} from './subagent-preset-presentation.js';
import { statusBadgeVariant } from './settings-status-badge.js';

/** The preset as the form holds it: `thinkingLevel` gains the Selector's ''. */
type SubagentEditorDraft = Omit<SubagentPreset, 'thinkingLevel'> & {
  thinkingLevel: ThinkingLevel | '';
};

export function SubagentSettingsPage(props: {
  settings: AppSettings;
  connections: readonly LlmConnection[];
  onUpdate(
    patch: Parameters<typeof window.maka.settings.update>[0],
  ): Promise<UpdateAppSettingsResult>;
}) {
  const locale = useUiLocale();
  const copy = getSubagentSettingsCopy(locale);
  const toast = useToast();
  const [route, setRoute] = useState<SubagentPageRoute>({ kind: 'list' });
  const [saving, setSaving] = useState(false);
  const presets = props.settings.subagents.presets;
  const { level, preset: editorPreset } = resolveSubagentRoute(route, presets);
  const atLimit = presets.length >= MAX_SUBAGENT_PRESETS;
  const addButtonRef = useRef<HTMLButtonElement>(null);
  const listReturnFocusRef = useRef<string | null>(null);
  const detailTitleId = useId();

  // The route the user is on has to agree with the level being rendered, or a
  // preset that reappears under the same id (an undo elsewhere, a re-import)
  // would re-open an editor the user already left.
  useEffect(() => {
    if (level === 'list' && route.kind !== 'list') setRoute({ kind: 'list' });
  }, [level, route.kind]);

  useSettingsRouteFocus({
    level,
    listLevel: 'list',
    // The level, not its back button: an IconButton opens its tooltip on
    // focus, so focusing one on arrival would pop a tooltip at every mouse
    // user who merely clicked a row.
    focusSelectors: () => ['[data-maka-contract="subagent-detail"]'],
    listReturnFocusRef,
    listReturnSelector: (id) => `[data-subagent-preset="${CSS.escape(id)}"]`,
    listFallbackRef: addButtonRef,
  });

  function openEditor(presetId: string): void {
    listReturnFocusRef.current = presetId;
    setRoute({ kind: 'edit', presetId });
  }

  /**
   * `expectPresent` is the whole point of reading the result back. Settings
   * normalization DROPS a preset it dislikes instead of rejecting the write
   * (`normalizeSubagentSettings`), so a resolved promise is not a saved
   * preset: a name past the limit, or a 65th preset created while the list
   * filled up elsewhere, would otherwise report success and return the user
   * to a list that does not contain what they just saved.
   */
  async function persist(
    nextPresets: SubagentPreset[],
    expectPresent?: string,
  ): Promise<boolean> {
    setSaving(true);
    try {
      const result = await props.onUpdate({ subagents: { presets: nextPresets } });
      if (
        expectPresent !== undefined &&
        !result.settings.subagents.presets.some((candidate) => candidate.id === expectPresent)
      ) {
        toast.error(copy.toast.saveFailed, copy.toast.rejected);
        return false;
      }
      return true;
    } catch (error) {
      toast.error(copy.toast.saveFailed, settingsActionErrorMessage(error, locale));
      return false;
    } finally {
      setSaving(false);
    }
  }

  async function removePreset(preset: SubagentPreset): Promise<void> {
    const confirmed = await toast.confirm({
      title: copy.remove.title(preset.name),
      description: copy.remove.description,
      confirmLabel: copy.remove.confirm,
      cancelLabel: copy.remove.cancel,
      destructive: true,
    });
    if (!confirmed) return;
    // No `setRoute` on success: the preset is gone, so the edit route is
    // unsatisfiable and the effect above commits the list for both this path
    // and a deletion that happened somewhere else.
    await persist(presets.filter((candidate) => candidate.id !== preset.id));
  }

  if (level !== 'list') {
    return (
      // A named region, so a screen reader announces which preset the user
      // landed in. tabIndex -1 so the level itself can take the focus the
      // route-focus hook hands it; it draws no ring.
      <VStack
        gap={5}
        tabIndex={-1}
        role="region"
        aria-labelledby={detailTitleId}
        className="settingsRouteLevel"
        data-maka-contract="subagent-detail"
      >
        <SettingsRouteHeader
          onBack={() => setRoute({ kind: 'list' })}
          backLabel={copy.editor.backToList}
          titleId={detailTitleId}
          title={editorPreset ? editorPreset.name : copy.section.add}
          subtitle={editorPreset ? copy.editor.editSubtitle : copy.editor.createSubtitle}
        />
        <SubagentPresetEditor
          key={editorPreset?.id ?? 'new'}
          preset={editorPreset}
          presets={presets}
          connections={props.connections}
          isSaving={saving}
          onCancel={() => setRoute({ kind: 'list' })}
          onDelete={editorPreset ? () => void removePreset(editorPreset) : undefined}
          onSave={async (next) => {
            const nextPresets = editorPreset
              ? presets.map((candidate) => candidate.id === editorPreset.id ? next : candidate)
              : [...presets, next];
            if (await persist(nextPresets, next.id)) setRoute({ kind: 'list' });
          }}
        />
      </VStack>
    );
  }

  return (
    <SettingsPage>
      <SettingsSection
        title={copy.section.title}
        description={copy.section.count(presets.length, MAX_SUBAGENT_PRESETS)}
        action={presets.length > 0 ? (
          <Button
            ref={addButtonRef}
            variant="primary"
            size="sm"
            label={copy.section.add}
            isDisabled={saving || atLimit}
            onClick={() => setRoute({ kind: 'create' })}
          />
        ) : undefined}
      >
        {presets.length === 0 ? (
          // The empty state owns the only call to action here; a section
          // action beside it would be the same button twice on one screen.
          <EmptyState
            title={copy.section.emptyTitle}
            description={copy.section.emptyDescription}
            actions={(
              <Button
                ref={addButtonRef}
                variant="primary"
                label={copy.section.add}
                isDisabled={saving}
                onClick={() => setRoute({ kind: 'create' })}
              />
            )}
          />
        ) : presets.map((preset) => {
          const availability = subagentPresetAvailability(preset, props.connections);
          // A row states what the preset is for; its route, its id, and its
          // capability boundary are the editor's answer, one level in. Only a
          // route the main agent cannot take carries a badge — 已停用 would be
          // the switch beside it said twice, and a list where every row says
          // 可用 says nothing.
          const problem = {
            available: null,
            disabled: null,
            missing_connection: copy.status.missingConnection,
            connection_disabled: copy.status.connectionDisabled,
            model_disabled: copy.status.modelDisabled,
          }[availability.kind];
          return (
            <SettingsRow
              key={preset.id}
              align="start"
              label={(
                <HStack gap={2} vAlign="center" wrap="wrap">
                  <span>{preset.name}</span>
                  {problem ? (
                    <Badge variant={statusBadgeVariant(availability.tone)} label={problem} />
                  ) : null}
                </HStack>
              )}
              description={preset.description || copy.row.fallbackDescription}
              end={(
                <>
                  <Switch
                    label={`${copy.row.enabled}: ${preset.name}`}
                    isLabelHidden
                    value={preset.enabled}
                    isDisabled={saving}
                    onChange={(enabled) => {
                      void persist(
                        presets.map((candidate) =>
                          candidate.id === preset.id ? { ...candidate, enabled } : candidate,
                        ),
                      );
                    }}
                  />
                  <IconButton
                    variant="ghost"
                    size="sm"
                    label={copy.row.configure(preset.name)}
                    tooltip={copy.row.configure(preset.name)}
                    icon={<ChevronRight size={16} aria-hidden="true" />}
                    isDisabled={saving}
                    // The focus anchor for the way back.
                    data-subagent-preset={preset.id}
                    onClick={() => openEditor(preset.id)}
                  />
                </>
              )}
            />
          );
        })}
      </SettingsSection>
    </SettingsPage>
  );
}

function SubagentPresetEditor(props: {
  preset: SubagentPreset | null;
  presets: readonly SubagentPreset[];
  connections: readonly LlmConnection[];
  isSaving: boolean;
  onCancel(): void;
  onDelete?(): void;
  onSave(preset: SubagentPreset): Promise<void>;
}) {
  const locale = useUiLocale();
  const copy = getSubagentSettingsCopy(locale);
  const usableConnections = useMemo(
    () => props.connections.filter((connection) => connection.enabled),
    [props.connections],
  );
  const existingIds = useMemo(
    () => new Set(props.presets.filter((preset) => preset.id !== props.preset?.id).map((preset) => preset.id)),
    [props.preset?.id, props.presets],
  );
  const initialConnection = props.preset
    ? props.connections.find((connection) => connection.slug === props.preset?.connectionSlug)
    : usableConnections[0];
  const initialModels = initialConnection && initialConnection.enabled
    ? connectionEnabledModelIds(initialConnection)
    : [];
  const [draft, setDraft] = useState<SubagentEditorDraft>(() => ({
    // Empty, not a pre-derived `subagent`: an id the user has not been asked
    // for yet reads as a value the page already decided. Typing the name fills
    // it (until the user takes it over), and the placeholder says what it is.
    id: props.preset?.id ?? '',
    name: props.preset?.name ?? '',
    description: props.preset?.description ?? '',
    profile: props.preset?.profile ?? 'local_read',
    connectionSlug: props.preset?.connectionSlug ?? usableConnections[0]?.slug ?? '',
    model: props.preset?.model ?? initialModels[0] ?? '',
    thinkingLevel: props.preset?.thinkingLevel ?? '',
    enabled: props.preset?.enabled ?? true,
  }));
  const [idWasEdited, setIdWasEdited] = useState(false);
  const [submitted, setSubmitted] = useState(false);
  const selectedConnection = props.connections.find(
    (connection) => connection.slug === draft.connectionSlug,
  );
  const enabledModels = selectedConnection?.enabled
    ? connectionEnabledModelIds(selectedConnection)
    : [];
  const thinkingLevels = selectedConnection
    ? thinkingVariantsForModel(selectedConnection.providerType, draft.model)
    : [];
  const profileCopy = copy.profiles[draft.profile];
  const validId = isSafeSubagentPresetId(draft.id.trim());
  const duplicateId = existingIds.has(draft.id.trim());
  const validConnection = Boolean(selectedConnection?.enabled);
  const validModel = enabledModels.includes(draft.model);
  const canSave = Boolean(
    draft.name.trim() &&
    (props.preset !== null || (validId && !duplicateId)) &&
    validConnection &&
    validModel,
  );
  const connectionOptions = props.connections.map((connection) => ({
    value: connection.slug,
    label: connection.name,
    disabled: !connection.enabled,
  }));
  if (
    draft.connectionSlug &&
    !props.connections.some((connection) => connection.slug === draft.connectionSlug)
  ) {
    connectionOptions.unshift({
      value: draft.connectionSlug,
      label: `${draft.connectionSlug} · ${copy.status.missingConnection}`,
      disabled: true,
    });
  }
  const modelOptions: SelectorOptionData[] = enabledModels.map((model) => ({
    value: model,
    label: model,
  }));
  if (draft.model && !enabledModels.includes(draft.model)) {
    modelOptions.unshift({
      value: draft.model,
      label: `${draft.model} · ${copy.status.modelDisabled}`,
      disabled: true,
    });
  }

  function updateName(value: string): void {
    // Capped at the one place the name changes, because the store DROPS a
    // preset whose name is over the limit rather than trimming it — an error
    // message would arrive after the row had already disappeared, and Astryx's
    // TextInput has no maxLength to lean on.
    const name = value.slice(0, SUBAGENT_PRESET_NAME_MAX_CHARS);
    setDraft((current) => nextSubagentDraftForName(current, name, idWasEdited, existingIds));
  }

  function selectConnection(connectionSlug: string): void {
    const connection = usableConnections.find((candidate) => candidate.slug === connectionSlug);
    const models = connection ? connectionEnabledModelIds(connection) : [];
    setDraft((current) => ({
      ...current,
      connectionSlug,
      model: models[0] ?? '',
      thinkingLevel: '',
    }));
  }

  async function submit(): Promise<void> {
    setSubmitted(true);
    if (!canSave) return;
    await props.onSave({
      // An existing preset's id comes from the preset, never from the draft:
      // session history and the main agent's routing key on it, and the field
      // is not even rendered in this branch.
      id: props.preset ? props.preset.id : draft.id.trim(),
      name: draft.name.trim(),
      description: draft.description.trim(),
      profile: draft.profile,
      connectionSlug: draft.connectionSlug,
      model: draft.model,
      ...(draft.thinkingLevel ? { thinkingLevel: draft.thinkingLevel } : {}),
      enabled: draft.enabled,
    });
  }

  const idStatus = submitted && !validId
    ? { type: 'error' as const, message: copy.editor.invalidId }
    : submitted && duplicateId
      ? { type: 'error' as const, message: copy.editor.duplicateId }
      : undefined;

  return (
    <SettingsPage>
      {/* Name and guidance are prose the user writes, so they are full-width
          fields, not values crammed into a row's end slot. */}
      <SettingsSection title={copy.editor.groupPurpose} description={copy.editor.groupPurposeHelp}>
        <SettingsField>
          <TextInput
            label={copy.editor.name}
            value={draft.name}
            placeholder={copy.editor.namePlaceholder}
            isDisabled={props.isSaving}
            status={submitted && !draft.name.trim()
              ? { type: 'error', message: copy.editor.requiredName }
              : undefined}
            onChange={updateName}
          />
        </SettingsField>
        <SettingsField>
          {/* Optional, because the stored contract makes it optional and the
              list has a fallback line for exactly that. Requiring it here made
              a legal preset uneditable until the user wrote prose. */}
          <TextArea
            label={copy.editor.description}
            value={draft.description}
            placeholder={copy.editor.descriptionPlaceholder}
            rows={3}
            // The counter is Astryx's; the cap is ours, because `maxLength`
            // only styles the counter and the store truncates silently.
            maxLength={SUBAGENT_PRESET_DESCRIPTION_MAX_CHARS}
            isDisabled={props.isSaving}
            onChange={(description) => setDraft((current) => ({
              ...current,
              description: description.slice(0, SUBAGENT_PRESET_DESCRIPTION_MAX_CHARS),
            }))}
          />
        </SettingsField>
        {/* An existing id is a settled fact — session history and the main
            agent's routing both reference it — so it reads as a row's value.
            Only a new preset's id is still the user's to type. */}
        {props.preset ? (
          <SettingRow
            title={copy.editor.id}
            detail={copy.editor.idDescription}
            value={props.preset.id}
            mono
          />
        ) : (
          <SettingsField>
            <TextInput
              label={copy.editor.id}
              description={copy.editor.idDescription}
              value={draft.id}
              placeholder={copy.editor.idPlaceholder}
              isDisabled={props.isSaving}
              status={idStatus}
              onChange={(id) => {
                setIdWasEdited(true);
                setDraft((current) => ({ ...current, id }));
              }}
            />
          </SettingsField>
        )}
      </SettingsSection>

      <SettingsSection title={copy.editor.groupRoute} description={copy.editor.groupRouteHelp}>
        <SettingsRow
          label={copy.editor.profile}
          description={profileCopy.description}
          align="start"
          end={(
            <Selector
              label={copy.editor.profile}
              isLabelHidden
              value={draft.profile}
              options={(Object.keys(copy.profiles) as SubagentProfile[]).map((profile) => ({
                value: profile,
                label: copy.profiles[profile].label,
              }))}
              width="100%"
              isDisabled={props.isSaving}
              onChange={(profile) => setDraft((current) => ({
                ...current,
                profile: profile as SubagentProfile,
              }))}
            />
          )}
        />
        {draft.profile === 'implementation' ? (
          <SettingsField>
            <Banner status="warning" title={copy.editor.implementationWarning} />
          </SettingsField>
        ) : null}
        <SettingsRow
          label={copy.editor.connection}
          end={(
            <Selector
              label={copy.editor.connection}
              isLabelHidden
              value={draft.connectionSlug}
              options={connectionOptions}
              width="100%"
              isDisabled={props.isSaving || usableConnections.length === 0}
              disabledMessage={usableConnections.length === 0 ? copy.editor.noConnection : undefined}
              status={submitted && !validConnection
                ? { type: 'error', message: copy.editor.invalidConnection }
                : usableConnections.length === 0
                  ? { type: 'warning', message: copy.editor.noConnection }
                  : undefined}
              onChange={selectConnection}
            />
          )}
        />
        <SettingsRow
          label={copy.editor.model}
          end={(
            <Selector
              label={copy.editor.model}
              isLabelHidden
              value={draft.model}
              options={modelOptions}
              width="100%"
              isDisabled={props.isSaving || enabledModels.length === 0}
              disabledMessage={enabledModels.length === 0 ? copy.editor.noModel : undefined}
              // The route is two choices, so it gets two errors: an enabled
              // connection with no model selected is the model's problem.
              status={submitted && validConnection && !validModel
                ? { type: 'error', message: copy.editor.invalidModel }
                : undefined}
              onChange={(model) => setDraft((current) => ({ ...current, model, thinkingLevel: '' }))}
            />
          )}
        />
        {thinkingLevels.length > 0 ? (
          <SettingsRow
            label={copy.editor.thinking}
            end={(
              <Selector
                label={copy.editor.thinking}
                isLabelHidden
                value={thinkingLevels.includes(draft.thinkingLevel as ThinkingLevel)
                  ? draft.thinkingLevel
                  : ''}
                options={[
                  { value: '', label: copy.editor.defaultThinking },
                  ...thinkingLevels.map((level) => ({ value: level, label: copy.thinking[level] })),
                ]}
                width="100%"
                isDisabled={props.isSaving}
                onChange={(thinkingLevel) => setDraft((current) => ({
                  ...current,
                  thinkingLevel: thinkingLevel as ThinkingLevel | '',
                }))}
              />
            )}
          />
        ) : null}
        <SettingsRow
          label={copy.editor.enabled}
          description={copy.editor.enabledDescription}
          align="start"
          end={(
            <Switch
              label={copy.editor.enabled}
              isLabelHidden
              value={draft.enabled}
              isDisabled={props.isSaving}
              onChange={(enabled) => setDraft((current) => ({ ...current, enabled }))}
            />
          )}
        />
      </SettingsSection>

      {/* Save commits the whole preset, not the route group it would sit
          inside, so it stands on its own between the groups and the delete
          section. */}
      <HStack gap={2} wrap="wrap">
          <Button
            variant="primary"
            label={props.isSaving
              ? copy.editor.saving
              : props.preset
                ? copy.editor.save
                : copy.editor.create}
            isDisabled={props.isSaving}
            onClick={() => void submit()}
          />
          <Button
            variant="ghost"
            label={copy.editor.cancel}
            isDisabled={props.isSaving}
            onClick={props.onCancel}
          />
      </HStack>

      {/* Deletion is last and stands alone, so a mis-aimed cursor has nothing
          quiet to hit beside it — the providers detail answers it the same way. */}
      {props.onDelete ? (
        <SettingsSection title={copy.editor.dangerZone} description={copy.editor.dangerZoneHelp}>
          <SettingsActions>
            <Button
              variant="destructive"
              label={copy.editor.delete}
              isDisabled={props.isSaving}
              onClick={props.onDelete}
            />
          </SettingsActions>
        </SettingsSection>
      ) : null}
    </SettingsPage>
  );
}
