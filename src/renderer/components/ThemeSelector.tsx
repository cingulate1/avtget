import { useThemeStore, themes, ThemeMode } from '../store/themeStore';

const themeOptions: { mode: ThemeMode; color: string; borderColor: string }[] = [
    { mode: 'light', color: '#ffffff', borderColor: '#2563eb' },  // white with blue border
    { mode: 'navy', color: '#0f172a', borderColor: '#ec4899' },   // navy with pink border
    { mode: 'dark', color: '#000000', borderColor: '#22c55e' },   // black with green border
];

export function ThemeSelector() {
    const currentTheme = useThemeStore((s) => s.theme);
    const setTheme = useThemeStore((s) => s.setTheme);

    return (
        <div className="flex gap-1.5 items-center">
            {themeOptions.map(({ mode, color, borderColor }) => (
                <button
                    key={mode}
                    onClick={() => setTheme(mode)}
                    className="w-4 h-4 rounded-full transition-transform duration-200 hover:scale-110"
                    style={{
                        backgroundColor: color,
                        border: `2px solid ${borderColor}`,
                        boxShadow: currentTheme === mode ? `0 0 0 2px ${borderColor}40` : 'none',
                    }}
                    title={`${mode.charAt(0).toUpperCase() + mode.slice(1)} mode`}
                    aria-label={`Switch to ${mode} mode`}
                />
            ))}
        </div>
    );
}
