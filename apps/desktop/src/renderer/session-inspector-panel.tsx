import { useMemo, useState } from 'react';
import { Badge } from '@astryxdesign/core/Badge';
import { Banner } from '@astryxdesign/core/Banner';
import { Button } from '@astryxdesign/core/Button';
import { EmptyState } from '@astryxdesign/core/EmptyState';
import { HStack, VStack } from '@astryxdesign/core/Layout';
import { MetadataList, MetadataListItem } from '@astryxdesign/core/MetadataList';
import { Section } from '@astryxdesign/core/Section';
import { Switch } from '@astryxdesign/core/Switch';
import { Text } from '@astryxdesign/core/Text';
import { TextInput } from '@astryxdesign/core/TextInput';
import { useUiLocale } from '@maka/ui';
import { Activity } from '@maka/ui/icons';
import { getDesktopConversationCopy } from './locales/conversation-copy.js';
import {
  applyInspectorFilter,
  type InspectorFilter,
} from './session-inspector-filter.js';
import {
  deriveInspectorPanelModel,
  type InspectorStepRow,
  type InspectorTurnRow,
} from './session-inspector-panel-model.js';
import { useSessionTrace } from './use-session-trace.js';

/**
 * Per-session causal timeline (#1625): what the session did, in order, with
 * what each model call cost and how long it took.
 *
 * Read-only. Every judgement it makes lives in `deriveInspectorPanelModel`;
 * this file lays the result out — in the same components the rest of the
 * workbar uses, so a read that failed looks like every other failed read
 * (Banner), and a session that did nothing looks like every other empty
 * surface (EmptyState) rather than like a stray paragraph.
 */
export function SessionInspectorPanel(props: { sessionId: string; active: boolean }) {
  const locale = useUiLocale();
  const copy = getDesktopConversationCopy(locale).inspector;
  const snapshot = useSessionTrace(props.sessionId, props.active, {
    loadFailed: copy.loadFailed,
    locale,
  });
  const [filter, setFilter] = useState<InspectorFilter>({});
  const model = useMemo(
    () => applyInspectorFilter(deriveInspectorPanelModel(snapshot.trace), filter),
    [snapshot.trace, filter],
  );
  const hidden = model.hiddenTurns + model.hiddenSteps;

  return (
    <Section
      variant="transparent"
      padding={3}
      className="maka-inspector-panel"
      data-maka-contract="session-inspector"
      aria-label={copy.ariaLabel}
      aria-busy={snapshot.loading || undefined}
    >
      <VStack gap={2} height="100%">
        {snapshot.error && (
          <Banner
            status="error"
            title={snapshot.error}
            endContent={
              <Button variant="ghost" size="sm" label={copy.retry} onClick={snapshot.retry} />
            }
          />
        )}

        {model.coverage && (
          <Banner
            status="warning"
            data-maka-contract="session-inspector-coverage"
            title={model.coverage.kind === 'absent' ? copy.coverageAbsent : copy.coveragePartial}
            description={
              [
                model.coverage.turnsMissing > 0 &&
                  `${model.coverage.turnsMissing} ${copy.turnsMissing}`,
                model.coverage.turnsShort > 0 && `${model.coverage.turnsShort} ${copy.turnsShort}`,
                model.coverage.unreadableRecords > 0 &&
                  `${model.coverage.unreadableRecords} ${copy.unreadable}`,
              ]
                .filter(Boolean)
                .join(' · ') || undefined
            }
          />
        )}

        {!model.empty && (
          <MetadataList orientation="horizontal" label={{ position: 'top' }}>
            <MetadataListItem label={copy.totals.duration}>
              {formatDuration(model.totals.durationMs)}
            </MetadataListItem>
            <MetadataListItem label={copy.totals.calls}>
              {model.totals.modelAttempts}
            </MetadataListItem>
            {model.totals.retries > 0 && (
              <MetadataListItem label={copy.totals.retries}>
                {model.totals.retries}
              </MetadataListItem>
            )}
            {model.totals.compactions > 0 && (
              <MetadataListItem label={copy.totals.compactions}>
                {model.totals.compactions}
              </MetadataListItem>
            )}
            <MetadataListItem label={copy.totals.cost}>
              {formatCost(model.totals.costUsd, copy.costUnavailable)}
            </MetadataListItem>
          </MetadataList>
        )}

        {!model.empty && (
          <HStack gap={2} vAlign="center" wrap="wrap">
            <TextInput
              size="sm"
              label={copy.filterLabel}
              isLabelHidden
              hasClear
              value={filter.query ?? ''}
              placeholder={copy.filterPlaceholder}
              onChange={(value) => setFilter({ ...filter, query: value })}
            />
            <Switch
              label={copy.filterFailedOnly}
              value={filter.failedOnly ?? false}
              onChange={(checked) => setFilter({ ...filter, failedOnly: checked })}
            />
            {model.filtered && (
              <Button
                variant="ghost"
                size="sm"
                label={copy.filterClear}
                onClick={() => setFilter({})}
              />
            )}
          </HStack>
        )}

        {/* Three different silences, kept apart: a read that failed, a filter
            that matches nothing, and a session that did nothing. Only the last
            one is "nothing to trace".
            One persistent live region rather than three conditional ones: a
            container that mounts and unmounts is not announced, and these
            messages change as the reader types. */}
        <div
          role="status"
          aria-live="polite"
          className="maka-inspector-status"
          /* With nothing to trace the region IS the panel, so it takes the
             leftover height and centres its empty state the way every other
             workbar tab does. Carrying a hint beside a timeline, it hugs. */
          data-empty={model.empty || undefined}
        >
          {model.empty && !snapshot.loading && !snapshot.error && (
            <EmptyState
              isCompact
              title={copy.empty}
              icon={<Activity size={20} aria-hidden="true" />}
            />
          )}
          {!model.empty && model.turns.length === 0 && model.filtered && (
            <EmptyState
              isCompact
              title={copy.noMatches}
              data-maka-contract="session-inspector-no-matches"
            />
          )}
          {model.filtered && hidden > 0 && model.turns.length > 0 && (
            <Text
              type="supporting"
              color="secondary"
              data-maka-contract="session-inspector-hidden"
            >
              {hidden} {copy.hiddenByFilter}
            </Text>
          )}
        </div>

        <ol className="maka-inspector-turns">
          {model.turns.map((turn) => (
            <TurnRow
              key={turn.turnId}
              turn={turn}
              costUnavailable={copy.costUnavailable}
              failedLabel={copy.turnFailed}
              recoveredLabel={copy.recovered}
            />
          ))}
        </ol>
      </VStack>
    </Section>
  );
}

function TurnRow(props: {
  turn: InspectorTurnRow;
  costUnavailable: string;
  failedLabel: string;
  recoveredLabel: string;
}) {
  const { turn } = props;
  return (
    <li className="maka-inspector-turn" data-maka-contract="session-inspector-turn">
      <HStack gap={2} vAlign="center" wrap="wrap">
        <Text type="label" weight="semibold">
          {turn.turnId}
        </Text>
        <Text type="supporting" color="secondary">
          {formatDuration(turn.durationMs)}
        </Text>
        <Text type="supporting" color="secondary" className="maka-inspector-cost">
          {formatCost(turn.totals.costUsd, props.costUnavailable)}
        </Text>
        {turn.failed && (
          <Badge
            variant="error"
            data-maka-contract="session-inspector-turn-failed"
            label={turn.failureCode ? `${props.failedLabel} · ${turn.failureCode}` : props.failedLabel}
          />
        )}
      </HStack>
      <ol className="maka-inspector-steps">
        {turn.steps.map((step) => (
          <StepRow
            key={step.id}
            step={step}
            costUnavailable={props.costUnavailable}
            recoveredLabel={props.recoveredLabel}
          />
        ))}
      </ol>
    </li>
  );
}

function StepRow(props: {
  step: InspectorStepRow;
  costUnavailable: string;
  recoveredLabel: string;
}) {
  const { step } = props;
  return (
    <li
      className="maka-inspector-step"
      data-maka-contract="session-inspector-step"
      data-kind={step.kind}
      data-failed={step.failed || undefined}
    >
      <Text type="supporting" color="secondary" className="maka-inspector-step-kind">
        {step.kind}
      </Text>
      <Text type="supporting">{step.label}</Text>
      {step.detail && (
        <Text type="supporting" color="secondary">
          {step.detail}
        </Text>
      )}
      {step.recovered && (
        <Text type="supporting" color="secondary">
          {props.recoveredLabel}: {step.recovered}
        </Text>
      )}
      {step.retries !== undefined && (
        <Text type="supporting" color="secondary" className="maka-inspector-cost">
          ×{step.retries + 1}
        </Text>
      )}
      {step.durationMs !== undefined && (
        <Text type="supporting" color="secondary">
          {formatDuration(step.durationMs)}
        </Text>
      )}
      {step.kind === 'model_call' && (
        <Text type="supporting" color="secondary" className="maka-inspector-cost">
          {formatCost(step.costUsd, props.costUnavailable)}
        </Text>
      )}
    </li>
  );
}

function formatDuration(ms: number): string {
  if (ms < 1_000) return `${Math.round(ms)}ms`;
  if (ms < 60_000) return `${(ms / 1_000).toFixed(1)}s`;
  return `${Math.floor(ms / 60_000)}m${Math.round((ms % 60_000) / 1_000)}s`;
}

/**
 * Absent cost renders as words, never as `$0.00`: the canonical record keeps
 * "nobody could price this" and "this was free" apart, and so does the panel.
 */
function formatCost(costUsd: number | undefined, unavailable: string): string {
  if (costUsd === undefined) return unavailable;
  return costUsd < 0.01 ? `$${costUsd.toFixed(4)}` : `$${costUsd.toFixed(2)}`;
}
