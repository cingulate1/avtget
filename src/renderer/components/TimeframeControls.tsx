// Unit codes shared with the backend's `parse_timeframe_days` (d=1, w=7, m=30).
export type TimeframeUnit = 'd' | 'w' | 'm';
// The "from" side adds a "today" sentinel (no lower offset → scrape up to now).
export type FromUnit = 'today' | TimeframeUnit;

interface TimeframeControlsProps {
  fromValue: number;
  fromUnit: FromUnit;
  toValue: number;
  toUnit: TimeframeUnit;
  onFromValueChange: (value: number) => void;
  onFromUnitChange: (unit: FromUnit) => void;
  onToValueChange: (value: number) => void;
  onToUnitChange: (unit: TimeframeUnit) => void;
}

// Channel scrape window as a half-open range of distances-from-today:
//   From: the near edge (default "today" → no upper date bound)
//   To:   the far edge (default "2 weeks", the legacy single-window value)
// The parent enforces from <= to; this component is purely presentational.
export function TimeframeControls({
  fromValue,
  fromUnit,
  toValue,
  toUnit,
  onFromValueChange,
  onFromUnitChange,
  onToValueChange,
  onToUnitChange,
}: TimeframeControlsProps) {
  const isToday = fromUnit === 'today';

  return (
    <div className="flex items-center gap-2 px-3 py-2 bg-blue-50 border border-blue-200 rounded-md">
      <label className="text-sm font-medium text-gray-700">Timeframe:</label>

      <span className="text-sm font-medium text-gray-700">From:</span>
      <input
        type="number"
        // Empty + disabled while "today" is selected; activates to 1 otherwise.
        value={isToday ? '' : fromValue}
        onChange={(e) => onFromValueChange(parseInt(e.target.value) || 1)}
        min="1"
        disabled={isToday}
        className="w-16 px-2 py-1 border border-gray-300 rounded focus:outline-none focus:ring-2 focus:ring-blue-500 disabled:bg-gray-100 disabled:text-gray-400 disabled:cursor-not-allowed"
      />
      <select
        value={fromUnit}
        onChange={(e) => onFromUnitChange(e.target.value as FromUnit)}
        className="px-2 py-1 border border-gray-300 rounded focus:outline-none focus:ring-2 focus:ring-blue-500"
      >
        <option value="today">today</option>
        <option value="d">days</option>
        <option value="w">weeks</option>
        <option value="m">months</option>
      </select>

      <span className="text-sm font-medium text-gray-700 ml-2">To:</span>
      <input
        type="number"
        value={toValue}
        onChange={(e) => onToValueChange(parseInt(e.target.value) || 1)}
        min="1"
        className="w-16 px-2 py-1 border border-gray-300 rounded focus:outline-none focus:ring-2 focus:ring-blue-500"
      />
      <select
        value={toUnit}
        onChange={(e) => onToUnitChange(e.target.value as TimeframeUnit)}
        className="px-2 py-1 border border-gray-300 rounded focus:outline-none focus:ring-2 focus:ring-blue-500"
      >
        <option value="d">days</option>
        <option value="w">weeks</option>
        <option value="m">months</option>
      </select>
    </div>
  );
}
