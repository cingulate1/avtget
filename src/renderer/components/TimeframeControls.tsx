interface TimeframeControlsProps {
  value: number;
  unit: string;
  onValueChange: (value: number) => void;
  onUnitChange: (unit: string) => void;
}

export function TimeframeControls({
  value,
  unit,
  onValueChange,
  onUnitChange,
}: TimeframeControlsProps) {
  return (
    <div className="flex items-center gap-2 px-3 py-2 bg-blue-50 border border-blue-200 rounded-md">
      <label className="text-sm font-medium text-gray-700">Timeframe:</label>
      <input
        type="number"
        value={value}
        onChange={(e) => onValueChange(parseInt(e.target.value) || 1)}
        min="1"
        className="w-20 px-2 py-1 border border-gray-300 rounded focus:outline-none focus:ring-2 focus:ring-blue-500"
      />
      <select
        value={unit}
        onChange={(e) => onUnitChange(e.target.value)}
        className="px-2 py-1 border border-gray-300 rounded focus:outline-none focus:ring-2 focus:ring-blue-500"
      >
        <option value="d">days</option>
        <option value="w">weeks</option>
        <option value="m">months</option>
      </select>
    </div>
  );
}
