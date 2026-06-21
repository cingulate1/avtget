import { useState } from 'react';
import { InputFields } from './InputFields';
import { TimeframeControls } from './TimeframeControls';
import { EpisodeLimitControls } from './EpisodeLimitControls';
import { ActionButtons } from './ActionButtons';
import { useJobStore } from '../store/jobStore';

export function InputSection() {
  const inputs = useJobStore((s) => s.inputs);
  const setInputs = useJobStore((s) => s.setInputs);
  const [showTimeframe, setShowTimeframe] = useState(false);
  const [timeframeValue, setTimeframeValue] = useState(2);
  const [timeframeUnit, setTimeframeUnit] = useState('w');

  const showEpisodeLimit = useJobStore((s) => s.showEpisodeLimit);
  const setShowEpisodeLimit = useJobStore((s) => s.setShowEpisodeLimit);
  const episodeLimit = useJobStore((s) => s.episodeLimit);
  const setEpisodeLimit = useJobStore((s) => s.setEpisodeLimit);

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
          value={timeframeValue}
          unit={timeframeUnit}
          onValueChange={setTimeframeValue}
          onUnitChange={setTimeframeUnit}
        />
      )}
      {showEpisodeLimit && (
        <EpisodeLimitControls
          value={episodeLimit}
          onChange={setEpisodeLimit}
        />
      )}
      <ActionButtons
        timeframe={showTimeframe ? `${timeframeValue}${timeframeUnit}` : undefined}
      />
    </div>
  );
}
