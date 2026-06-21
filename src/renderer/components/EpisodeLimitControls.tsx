interface EpisodeLimitControlsProps {
  value: number;
  onChange: (value: number) => void;
}

export function EpisodeLimitControls({ value, onChange }: EpisodeLimitControlsProps) {
  const options = [
    { label: '1 episode', value: 1 },
    { label: '5 episodes', value: 5 },
    { label: '10 episodes', value: 10 },
    { label: '25 episodes', value: 25 },
    { label: '50 episodes', value: 50 },
    { label: 'All episodes', value: 0 },  // 0 = unlimited
  ];

  return (
    <div className="flex items-center gap-2 px-3 py-2 bg-purple-50 border border-purple-200 rounded-md">
      <label className="text-sm font-medium text-gray-700">Episodes:</label>
      <select
        value={value}
        onChange={(e) => onChange(parseInt(e.target.value))}
        className="px-2 py-1 border border-gray-300 rounded focus:outline-none focus:ring-2 focus:ring-purple-500"
      >
        {options.map((opt) => (
          <option key={opt.value} value={opt.value}>
            {opt.label}
          </option>
        ))}
      </select>
    </div>
  );
}
