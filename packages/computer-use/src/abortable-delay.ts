/**
 * Sleep that honours an abort signal.
 *
 * The model-facing `wait` action used a bare setTimeout, so a user stop during
 * a long wait was ignored until the timer fired on its own. That is a property
 * of the host's contract with the user, not of any one executor, so both
 * backends wait the same way.
 */
export function abortableDelay(ms: number, signal: AbortSignal): Promise<void> {
  if (ms <= 0) return Promise.resolve();
  return new Promise((resolve, reject) => {
    if (signal.aborted) {
      reject(signal.reason ?? new Error('aborted'));
      return;
    }
    const onAbort = (): void => {
      clearTimeout(timer);
      reject(signal.reason ?? new Error('aborted'));
    };
    const timer = setTimeout(() => {
      signal.removeEventListener('abort', onAbort);
      resolve();
    }, ms);
    signal.addEventListener('abort', onAbort, { once: true });
  });
}
