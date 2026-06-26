import { useState } from 'react';
import { InputFields } from './InputFields';
import { TimeframeControls, type TimeframeUnit, type FromUnit } from './TimeframeControls';
import { EpisodeLimitControls } from './EpisodeLimitControls';
import { ActionButtons } from './ActionButtons';
import { useJobStore } from '../store/jobStore';

// Day-equivalents for each unit — mirror the backend's `parse_timeframe_days`
// (months = 30 days) so the GUI's from/to comparison matches what the scrape
// actually does.
const UNIT_DAYS: Record<TimeframeUnit, number> = { d: 1, w: 7, m: 30 };
const toDays = (value: number, unit: TimeframeUnit) => value * UNIT_DAYS[unit];

interface TimeframeState {
  fromValue: number;
  fromUnit: FromUnit;
  toValue: number;
  toUnit: TimeframeUnit;
}

// Enforce the invariant from <= to by bumping `to` up to match `from` (value
// AND unit) whenever the from-distance would exceed it. "today" means from = 0
// days, which can never exceed a positive `to`, so it's always left alone. `to`
// is only ever pushed outward, never pulled in — so leaving the far edge wider
// than `from` (the common case) is preserved untouched.
function normalizeTimeframe(tf: TimeframeState): TimeframeState {
  if (tf.fromUnit === 'today') return tf;
  const fromDays = toDays(tf.fromValue, tf.fromUnit);
  if (fromDays > toDays(tf.toValue, tf.toUnit)) {
    return { ...tf, toValue: tf.fromValue, toUnit: tf.fromUnit };
  }
  return tf;
}

export function InputSection() {
  const inputs = useJobStore((s) => s.inputs);
  const setInputs = useJobStore((s) => s.setInputs);
  const [showTimeframe, setShowTimeframe] = useState(false);
  // Default window matches the legacy single control: today → 2 weeks back.
  const [tf, setTf] = useState<TimeframeState>({
    fromValue: 1,
    fromUnit: 'today',
    toValue: 2,
    toUnit: 'w',
  });

  const showEpisodeLimit = useJobStore((s) => s.showEpisodeLimit);
  const setShowEpisodeLimit = useJobStore((s) => s.setShowEpisodeLimit);
  const episodeLimit = useJobStore((s) => s.episodeLimit);
  const setEpisodeLimit = useJobStore((s) => s.setEpisodeLimit);

  const handleFromValueChange = (value: number) =>
    setTf((t) => normalizeTimeframe({ ...t, fromValue: value }));

  const handleFromUnitChange = (unit: FromUnit) =>
    setTf((t) => {
      // Switching back to "today" just deactivates the from-pair; `to` is left
      // exactly where the user had it.
      if (unit === 'today') return { ...t, fromUnit: 'today' };
      // Activating out of "today" seeds the value at 1; switching among
      // day/week/month keeps whatever value was already there.
      const fromValue = t.fromUnit === 'today' ? 1 : t.fromValue;
      return normalizeTimeframe({ ...t, fromUnit: unit, fromValue });
    });

  const handleToValueChange = (value: number) =>
    setTf((t) => normalizeTimeframe({ ...t, toValue: value }));

  const handleToUnitChange = (unit: TimeframeUnit) =>
    setTf((t) => normalizeTimeframe({ ...t, toUnit: unit }));

  // Compose the wire strings only when the timeframe panel is active. `to` is
  // the legacy `timeframe` field (far edge); `from` is the new near edge and is
  // omitted entirely when "today" (no upper date bound).
  const toTimeframe = `${tf.toValue}${tf.toUnit}`;
  const fromTimeframe =
    tf.fromUnit === 'today' ? undefined : `${tf.fromValue}${tf.fromUnit}`;

  return (
    <div className="flex flex-col gap-2">
      <InputFields
        inputs={inputs}
        setInputs={setInputs}
        setShowTimeframe={setShowTimeframe}
        setShowEpisodeLimit={setShowEpisodeLimit}
      />
      {showTimeframe && (
        <TimeframeControls
          fromValue={tf.fromValue}
          fromUnit={tf.fromUnit}
          toValue={tf.toValue}
          toUnit={tf.toUnit}
          onFromValueChange={handleFromValueChange}
          onFromUnitChange={handleFromUnitChange}
          onToValueChange={handleToValueChange}
          onToUnitChange={handleToUnitChange}
        />
      )}
      {showEpisodeLimit && (
        <EpisodeLimitControls
          value={episodeLimit}
          onChange={setEpisodeLimit}
        />
      )}
      <ActionButtons
        timeframe={showTimeframe ? toTimeframe : undefined}
        timeframeFrom={showTimeframe ? fromTimeframe : undefined}
      />
    </div>
  );
}
