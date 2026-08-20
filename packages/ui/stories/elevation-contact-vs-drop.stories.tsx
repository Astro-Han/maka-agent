/**
 * The oracle for the `--shadow-low` change in #feat/theme-elevation-floating-shadow.
 *
 * A token diff is unreviewable as text: `0 2px 4px` versus `0 0 2px` says
 * nothing about whether the surface reads as resting on the page or stuck to
 * it. The two values have to sit side by side, on the same fill, at the same
 * radius, in both modes — otherwise the only way to judge the change is to
 * flip between two builds from memory.
 *
 * DROP is the stock Astryx neutral value, hardcoded here so this story keeps
 * showing the comparison after the token itself has moved. CONTACT reads
 * `var(--shadow-low)` live, so it tracks whatever the theme currently ships and
 * this story cannot silently drift away from the thing it is reviewing.
 *
 * Laid out to fit ~760px so the pair can be read at 1:1. The whole difference
 * lives in a few px of gradient at the edge of each shape; a viewport that
 * forces the browser to downsample the screenshot destroys exactly the signal
 * this story exists to show.
 *
 * Delete when the ramp decision is settled and `med`/`high` have followed —
 * at that point the comparison is history, not a review surface.
 */
import type { Meta, StoryObj } from '@storybook/react-vite';

/** Astryx neutral's stock `--shadow-low`, frozen at the value this change replaces. */
const DROP =
  '0 2px 4px light-dark(oklch(0 0 0 / 5%), oklch(0 0 0 / 25%)), ' +
  '0 4px 8px light-dark(oklch(0 0 0 / 10%), oklch(0 0 0 / 40%)), ' +
  'inset 0 0 0 1px light-dark(transparent, oklch(1 0 0 / 8%))';

/** Whatever the theme ships right now. */
const CONTACT = 'var(--shadow-low)';

const COL = 320;
const GAP = 48;

const meta = {
  title: 'Design System/Elevation — Contact vs Drop',
  parameters: { layout: 'fullscreen' },
} satisfies Meta;

export default meta;

type Story = StoryObj<typeof meta>;

function Surface({
  shadow,
  radius,
  width,
  height,
  children,
}: {
  shadow: string;
  radius: number;
  width: number;
  height: number;
  children?: React.ReactNode;
}) {
  return (
    <div
      style={{
        width,
        height,
        borderRadius: radius,
        boxShadow: shadow,
        background: 'var(--color-background-surface)',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        color: 'var(--color-text-secondary, inherit)',
        fontSize: 13,
      }}>
      {children}
    </div>
  );
}

/** One shape, drawn twice under the two shadows, in the two columns. */
function Row({
  label,
  radius,
  width,
  height,
  children,
}: {
  label: string;
  radius: number;
  width: number;
  height: number;
  children?: React.ReactNode;
}) {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
      <div style={{ fontSize: 11, opacity: 0.5, letterSpacing: 0.3 }}>{label}</div>
      <div style={{ display: 'flex', gap: GAP }}>
        <div style={{ width: COL }}>
          <Surface shadow={DROP} radius={radius} width={width} height={height}>
            {children}
          </Surface>
        </div>
        <div style={{ width: COL }}>
          <Surface shadow={CONTACT} radius={radius} width={width} height={height}>
            {children}
          </Surface>
        </div>
      </div>
    </div>
  );
}

function Matrix() {
  return (
    <div
      style={{
        // The plate. Elevation is only legible against the surface it sits on,
        // and `body` is the token the shell actually paints behind these.
        background: 'var(--color-background-body)',
        padding: 32,
        minHeight: '100vh',
        display: 'flex',
        flexDirection: 'column',
        gap: 32,
      }}>
      <div style={{ display: 'flex', gap: GAP }}>
        <div style={{ width: COL, fontSize: 12, fontWeight: 600, letterSpacing: 0.4 }}>
          DROP — Astryx neutral stock
        </div>
        <div style={{ width: COL, fontSize: 12, fontWeight: 600, letterSpacing: 0.4 }}>
          CONTACT — var(--shadow-low)
        </div>
      </div>

      <Row label="Composer" radius={28} width={COL} height={96}>
        Ask Maka…
      </Row>
      <Row label="Card" radius={12} width={COL} height={72} />
      <Row label="Floating capsule" radius={9999} width={132} height={40} />
      <Row label="Icon button" radius={9999} width={40} height={40} />

      <div style={{ fontSize: 12, opacity: 0.55, maxWidth: COL * 2 + GAP, lineHeight: 1.6 }}>
        Toggle the Storybook dark mode to check both halves of every
        <code> light-dark() </code>
        pair. The tell to look for: DROP darkens below the shape and leaves its
        top edge flat against the plate; CONTACT darkens evenly around the edge
        and carries a lit top line, so the shape reads as having thickness.
      </div>
    </div>
  );
}

export const ContactVsDrop: Story = {
  render: () => <Matrix />,
};
