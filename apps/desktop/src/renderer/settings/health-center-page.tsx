import { useEffect, useState } from 'react';
import type {
  HealthSignal,
  HealthSignalLayer,
  HealthSnapshot,
} from '@maka/core';
import { HEALTH_SIGNAL_LAYERS } from '@maka/core';
import { Button, RelativeTime, StatusDot, useUiLocale, Banner } from '@maka/ui';
import { getHealthCenterCopy, type HealthCenterCopy } from '../locales/settings-health-copy';
import { settingsActionErrorMessage } from './settings-error-copy';
import { SettingsRow, SettingsSection } from './settings-section';
import { SettingsSkeletonStack } from './settings-skeleton';
import { statusBadgeVariant } from './settings-status-badge';

/**
 * PR-UI-9 — Health Center read-only page. Consumes `window.maka.health.getSnapshot()`
 * (shipped by @xuan PR-HC-1).
 *
 * Hard contract (per @xuan): "validation/config/permission/runtime 别聚成
 * 一个绿点". The UI groups signals by `layer` and renders each in its own
 * section so the user sees WHICH layer is okay and WHICH is degraded.
 *
 * Status semantics ≠ tone-by-color only. `ok` (validation pass) on an LLM
 * connection does NOT promote it to operational — that requires a runtime
 * probe in PR-REAL-4. The detail copy below makes the distinction explicit.
 *
 * Read-only boundary: no test buttons, no repair flows. Test/repair entries
 * will be wired in PR-HC-2 once typed actions are exposed.
*/
export function HealthCenterPage() {
  const locale = useUiLocale();
  const copy = getHealthCenterCopy(locale);
  const [snapshot, setSnapshot] = useState<HealthSnapshot | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [refreshTick, setRefreshTick] = useState(0);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    window.maka.health
      .getSnapshot()
      .then((next) => {
        if (cancelled) return;
        setSnapshot(next);
        setLoading(false);
      })
      .catch((err) => {
        if (cancelled) return;
        setError(settingsActionErrorMessage(err, locale));
        setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [locale, refreshTick]);

  if (loading) {
    return (
      <SettingsSkeletonStack label={copy.loading} />
    );
  }

  if (error || !snapshot) {
    return (
      <div className="settingsStructuredPage">
        <Banner
          status="error"
          title={copy.readFailed}
          description={error ?? copy.noData}
          endContent={<Button variant="primary" onClick={() => setRefreshTick((tick) => tick + 1)} label={copy.readAgain} />} />
      </div>
    );
  }

  const healthCheckedAtMs = snapshot.checkedAt;
  const signalsByLayer = groupSignalsByLayer(snapshot.signals);
  const blocksSendCount = snapshot.signals.filter((signal) => signal.blocksSend).length;
  const blocksCapabilityCount = snapshot.signals.filter((signal) => signal.blocksCapability).length;
  const summaryParts: Array<{ key: string; label: string; count: number; tone: 'neutral' | 'warning' | 'destructive' }> = [
    { key: 'ok', label: copy.statuses.ok.label, count: snapshot.summary.ok, tone: 'neutral' },
    { key: 'info', label: copy.statuses.info.label, count: snapshot.summary.info, tone: 'neutral' },
    { key: 'warning', label: copy.statuses.warning.label, count: snapshot.summary.warning, tone: 'warning' },
    { key: 'error', label: copy.statuses.error.label, count: snapshot.summary.error, tone: 'destructive' },
    { key: 'unknown', label: copy.statuses.unknown.label, count: snapshot.summary.unknown, tone: 'neutral' },
  ];

  return (
    <div className="settingsStructuredPage">
      <SettingsSection
        description={(
          <>
            {copy.subtitle} <strong>{copy.validationWarning}</strong>
          </>
        )}
        action={(
          <div className="settingsFormRowControlCluster">
            <small className="settingsHealthMetaLabel">
              {copy.lastRead}<RelativeTime ts={healthCheckedAtMs} className="settingsHelpInlineTime" />
            </small>
            <Button
              variant="secondary"
              size="sm"
              onClick={() => setRefreshTick((tick) => tick + 1)}
              label={copy.refresh}
            />
          </div>
        )}
      >
        <p className="settingsHealthSummaryLine" aria-label={copy.summaryAria}>
          {summaryParts.map((part) => (
            <span key={part.key} data-tone={part.count > 0 ? part.tone : 'neutral'}>
              {part.label} {part.count}
            </span>
          ))}
        </p>
      </SettingsSection>

      {blocksSendCount > 0 && (
        <Banner
          status="error"
          role="status"
          title={copy.blockers.send(blocksSendCount)}
          description={blocksCapabilityCount > 0 ? copy.blockers.capability(blocksCapabilityCount) : undefined}
        />
      )}
      {blocksSendCount === 0 && blocksCapabilityCount > 0 && (
        <Banner
          status="warning"
          role="status"
          title={copy.blockers.capability(blocksCapabilityCount)}
        />
      )}

      {HEALTH_SIGNAL_LAYERS.map((layer) => {
        const signals = signalsByLayer[layer];
        if (!signals || signals.length === 0) return null;
        const layerCopy = copy.layers[layer];
        return (
          <SettingsSection
            key={layer}
            title={layerCopy.label}
            description={layerCopy.description}
          >
            {signals.map((signal) => (
              <HealthSignalRow key={signal.id} signal={signal} copy={copy} />
            ))}
          </SettingsSection>
        );
      })}

      <p className="settingsHealthFootnote">
        {copy.footnote}
      </p>
    </div>
  );
}

function HealthSignalRow(props: { signal: HealthSignal; copy: HealthCenterCopy }) {
  const { signal, copy } = props;
  const statusCopy = copy.statuses[signal.status];
  const detail = copy.signalDetail(signal);
  const badgeVariant = statusBadgeVariant(statusCopy.tone);
  return (
    <SettingsRow
      align="start"
      label={(
        <span className="settingsHealthSignalIdentity">
            <strong className="settingsHealthSignalLabel">{copy.signalLabel(signal)}</strong>
            <small className="settingsHealthSignalScope">{copy.scopes[signal.scope]}</small>
        </span>
      )}
      description={(
        <span className="settingsHealthSignalDescription">
          <span>{copy.signalMessage(signal)}</span>
          {detail && <small>{detail}</small>}
          <span className="settingsHealthSignalMeta">
            <span>{copy.source}{copy.sources[signal.source]}</span>
            <span>{copy.checked}<RelativeTime ts={signal.checkedAt} className="settingsHelpInlineTime" /></span>
            {signal.blocksSend && <span data-tone="destructive">{copy.blocksSend}</span>}
            {signal.blocksCapability && <span data-tone="warning">{copy.blocksCapability}</span>}
          </span>
        </span>
      )}
      end={(
        <span className="settingsHealthSignalStatus">
          <StatusDot
            variant={badgeVariant === 'info' ? 'accent' : badgeVariant === 'success' ? 'success' : badgeVariant === 'warning' ? 'warning' : badgeVariant === 'error' ? 'error' : 'neutral'}
            label={statusCopy.label}
          />
          <span>{statusCopy.label}</span>
        </span>
      )}
    />
  );
}

function groupSignalsByLayer(signals: HealthSignal[]): Record<HealthSignalLayer, HealthSignal[]> {
  const byLayer: Record<HealthSignalLayer, HealthSignal[]> = {
    configuration: [],
    validation: [],
    permission: [],
    feature: [],
    action_approval: [],
    memory_acceptance: [],
    runtime_probe: [],
    storage: [],
  };
  for (const signal of signals) {
    byLayer[signal.layer].push(signal);
  }
  return byLayer;
}
