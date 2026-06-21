import { useState, useEffect, useCallback, useRef } from 'react';
import { useThemeStore, themes } from '../store/themeStore';
import { useSettingsStore } from '../store/settingsStore';

interface ClipRange {
    start: string;
    end: string;
}

interface ClipTimestampsProps {
    inputs: string[];
    clipTimestamps: ClipRange[][];
    setClipTimestamps: (timestamps: ClipRange[][]) => void;
}

// ── Segmented time input ────────────────────────────────────────────────────
// Adapted from Japanese Transcript Generator v3's time_input_widget.
// Stores value as total seconds internally, renders HH:MM:SS with
// individually-selectable, arrow-key-incrementable, digit-typeable segments.

type TimeSegment = 'hours' | 'minutes' | 'seconds';

const SEGMENT_DELTA: Record<TimeSegment, number> = {
    hours: 3600,
    minutes: 60,
    seconds: 1,
};
const SEGMENT_MAX: Record<TimeSegment, number> = {
    hours: 99,
    minutes: 59,
    seconds: 59,
};
const NEXT_SEG: Record<TimeSegment, TimeSegment> = {
    hours: 'minutes',
    minutes: 'seconds',
    seconds: 'hours',
};
const PREV_SEG: Record<TimeSegment, TimeSegment> = {
    hours: 'seconds',
    minutes: 'hours',
    seconds: 'minutes',
};
const SEGMENTS: TimeSegment[] = ['hours', 'minutes', 'seconds'];

function secsToHms(totalSecs: number): [number, number, number] {
    const h = Math.floor(totalSecs / 3600);
    const m = Math.floor((totalSecs % 3600) / 60);
    const s = totalSecs % 60;
    return [h, m, s];
}

function hmsToSecs(h: number, m: number, s: number): number {
    return h * 3600 + m * 60 + s;
}

function parseHmsString(raw: string): number {
    if (!raw) return 0;
    const parts = raw.split(':').map(Number);
    if (parts.length === 3) return hmsToSecs(parts[0] || 0, parts[1] || 0, parts[2] || 0);
    if (parts.length === 2) return hmsToSecs(0, parts[0] || 0, parts[1] || 0);
    return parts[0] || 0;
}

function formatHms(totalSecs: number): string {
    const [h, m, s] = secsToHms(totalSecs);
    return `${String(h).padStart(2, '0')}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`;
}

const MAX_SECS = 99 * 3600 + 59 * 60 + 59; // 99:59:59

interface TimeInputProps {
    value: number;
    onChange: (secs: number) => void;
    /** Called when Tab is pressed on the last segment — lets parent advance focus */
    onTabOut?: () => void;
    id?: string;
    themeColors: Record<string, string>;
}

function TimeInput({ value, onChange, onTabOut, id, themeColors }: TimeInputProps) {
    const [focusedSeg, setFocusedSeg] = useState<TimeSegment | null>(null);
    const [pendingDigit, setPendingDigit] = useState<number | null>(null);
    const containerRef = useRef<HTMLDivElement>(null);

    const [h, m, s] = secsToHms(value);
    const segValues: Record<TimeSegment, number> = { hours: h, minutes: m, seconds: s };

    // Focus management: clear focus when clicking outside
    useEffect(() => {
        if (focusedSeg === null) return;
        const handler = (e: MouseEvent) => {
            if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
                setFocusedSeg(null);
                setPendingDigit(null);
            }
        };
        document.addEventListener('mousedown', handler);
        return () => document.removeEventListener('mousedown', handler);
    }, [focusedSeg]);

    const writeSeg = useCallback((seg: TimeSegment, segVal: number) => {
        const clamped = Math.min(segVal, SEGMENT_MAX[seg]);
        const parts: [number, number, number] = [h, m, s];
        const idx = SEGMENTS.indexOf(seg);
        parts[idx] = clamped;
        onChange(Math.min(hmsToSecs(...parts), MAX_SECS));
    }, [h, m, s, onChange]);

    const handleKeyDown = useCallback((e: React.KeyboardEvent) => {
        if (focusedSeg === null) return;

        // Digit keys
        if (/^[0-9]$/.test(e.key)) {
            e.preventDefault();
            const d = Number(e.key);
            if (pendingDigit !== null) {
                // Second digit — combine, write, advance
                const combined = pendingDigit * 10 + d;
                writeSeg(focusedSeg, Math.min(combined, SEGMENT_MAX[focusedSeg]));
                setPendingDigit(null);
                if (focusedSeg === 'seconds') {
                    onTabOut?.();
                } else {
                    setFocusedSeg(NEXT_SEG[focusedSeg]);
                }
            } else {
                // First digit — write immediately, wait for second
                writeSeg(focusedSeg, d);
                setPendingDigit(d);
            }
            return;
        }

        switch (e.key) {
            case 'ArrowUp':
                e.preventDefault();
                setPendingDigit(null);
                onChange(Math.min(value + SEGMENT_DELTA[focusedSeg], MAX_SECS));
                break;
            case 'ArrowDown':
                e.preventDefault();
                setPendingDigit(null);
                onChange(Math.max(value - SEGMENT_DELTA[focusedSeg], 0));
                break;
            case 'ArrowLeft':
                e.preventDefault();
                setPendingDigit(null);
                setFocusedSeg(PREV_SEG[focusedSeg]);
                break;
            case 'ArrowRight':
                e.preventDefault();
                setPendingDigit(null);
                setFocusedSeg(NEXT_SEG[focusedSeg]);
                break;
            case 'Tab':
                e.preventDefault();
                setPendingDigit(null);
                if (focusedSeg === 'seconds') {
                    onTabOut?.();
                } else {
                    setFocusedSeg(NEXT_SEG[focusedSeg]);
                }
                break;
        }
    }, [focusedSeg, pendingDigit, value, onChange, onTabOut, writeSeg]);

    const isFocused = focusedSeg !== null;
    const borderColor = isFocused ? themeColors.buttonPrimary : themeColors.inputBorder;

    return (
        <div
            ref={containerRef}
            id={id}
            tabIndex={0}
            onKeyDown={handleKeyDown}
            onFocus={() => { if (focusedSeg === null) setFocusedSeg('hours'); }}
            className="inline-flex items-center rounded border px-2 py-1.5 cursor-text select-none outline-none"
            style={{
                backgroundColor: themeColors.cardBg,
                borderColor,
                fontFamily: 'monospace',
                fontSize: '0.875rem',
                lineHeight: '1.25rem',
            }}
        >
            {SEGMENTS.map((seg, i) => (
                <span key={seg} className="flex items-center">
                    {i > 0 && (
                        <span style={{ color: themeColors.textMuted }}>:</span>
                    )}
                    <span
                        onMouseDown={(e) => {
                            e.preventDefault();
                            setFocusedSeg(seg);
                            setPendingDigit(null);
                            containerRef.current?.focus();
                        }}
                        className="px-0.5 rounded-sm cursor-text"
                        style={{
                            backgroundColor: focusedSeg === seg ? themeColors.buttonPrimary : 'transparent',
                            color: focusedSeg === seg ? '#fff' : themeColors.text,
                        }}
                    >
                        {String(segValues[seg]).padStart(2, '0')}
                    </span>
                </span>
            ))}
        </div>
    );
}

// ── Main dialog ─────────────────────────────────────────────────────────────

export function ClipTimestampsDialog({ inputs, clipTimestamps, setClipTimestamps }: ClipTimestampsProps) {
    const [showDialog, setShowDialog] = useState(false);
    const currentTheme = useThemeStore((s) => s.theme);
    const themeColors = themes[currentTheme];
    const settings = useSettingsStore((s) => s.settings);
    const saveSettings = useSettingsStore((s) => s.saveSettings);

    // Filter to only show inputs that have content
    const validInputs = inputs.filter((input) => input.trim() !== '');

    // Ensure clipTimestamps array matches validInputs length
    useEffect(() => {
        if (clipTimestamps.length !== validInputs.length) {
            const newTimestamps = validInputs.map((_, i) =>
                clipTimestamps[i] || [{ start: '', end: '' }]
            );
            setClipTimestamps(newTimestamps);
        }
    }, [validInputs.length, clipTimestamps, setClipTimestamps]);

    const updateTimestamp = useCallback((
        urlIndex: number,
        clipIndex: number,
        field: 'start' | 'end',
        secs: number,
    ) => {
        const formatted = formatHms(secs);
        const newTimestamps = [...clipTimestamps];
        if (!newTimestamps[urlIndex]) {
            newTimestamps[urlIndex] = [{ start: '', end: '' }];
        }

        const clips = [...newTimestamps[urlIndex]];
        if (!clips[clipIndex]) {
            clips[clipIndex] = { start: '', end: '' };
        }

        clips[clipIndex] = { ...clips[clipIndex], [field]: formatted };

        // Setting the end time commits the displayed start (00:00:00) as a real value
        if (field === 'end' && !clips[clipIndex].start) {
            clips[clipIndex] = { ...clips[clipIndex], start: '00:00:00' };
        }

        // Auto-add new pair if both start and end are filled and this is the last pair
        const currentClip = clips[clipIndex];
        if (currentClip.start && currentClip.end && clipIndex === clips.length - 1) {
            clips.push({ start: '', end: '' });
        }

        // Remove empty trailing pairs (keep at least one)
        while (clips.length > 1) {
            const last = clips[clips.length - 1];
            const secondLast = clips[clips.length - 2];
            if (!last.start && !last.end && !secondLast.start && !secondLast.end) {
                clips.pop();
            } else {
                break;
            }
        }

        newTimestamps[urlIndex] = clips;
        setClipTimestamps(newTimestamps);
    }, [clipTimestamps, setClipTimestamps]);

    // Count how many clips are defined
    const clipCount = clipTimestamps.reduce((sum, clips) =>
        sum + (clips?.filter(c => c.start && c.end).length || 0), 0);

    return (
        <>
            {/* Button to open dialog */}
            <button
                onClick={() => setShowDialog(true)}
                className="px-3 py-2 text-sm rounded-md transition-transform duration-[80ms] hover:brightness-110 active:scale-95 active:brightness-90"
                style={{
                    backgroundColor: themeColors.buttonPrimary,
                    color: 'white',
                }}
            >
                Clips{clipCount > 0 ? ` (${clipCount})` : ''}
            </button>

            {/* Modal Dialog - wide like input fields */}
            {showDialog && (
                <div className="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50">
                    <div
                        className="rounded-lg p-6 mx-8 max-h-[80vh] overflow-auto"
                        style={{
                            backgroundColor: themeColors.cardBg,
                            width: 'calc(100% - 64px)',
                            maxWidth: '900px',
                        }}
                    >
                        <h2
                            className="text-xl font-bold mb-3"
                            style={{ color: themeColors.text }}
                        >
                            Set Clip Timestamps
                        </h2>

                        <label className="flex items-center gap-2 mb-4">
                            <input
                                type="checkbox"
                                checked={settings.default_clips_full_output}
                                onChange={(e) => {
                                    saveSettings({ ...settings, default_clips_full_output: e.target.checked });
                                }}
                                className="w-4 h-4"
                                style={{ accentColor: themeColors.buttonPrimary }}
                            />
                            <span
                                className="text-sm"
                                style={{ color: themeColors.text }}
                            >
                                Also output full media
                            </span>
                        </label>

                        {validInputs.length === 0 ? (
                            <p
                                className="text-base py-6 text-center"
                                style={{ color: themeColors.textMuted }}
                            >
                                Enter video URLs first to set clip timestamps
                            </p>
                        ) : (
                            <div className="space-y-4">
                                {validInputs.map((input, urlIndex) => {
                                    const clips = clipTimestamps[urlIndex] || [{ start: '', end: '' }];
                                    // Truncate display URL
                                    const displayUrl = input.length > 80 ? input.slice(0, 77) + '...' : input;

                                    return (
                                        <div
                                            key={urlIndex}
                                            className="p-3 rounded border"
                                            style={{
                                                backgroundColor: themeColors.inputBg,
                                                borderColor: themeColors.border,
                                            }}
                                        >
                                            {/* URL header */}
                                            <div
                                                className="text-sm mb-3 truncate"
                                                style={{ color: themeColors.textMuted }}
                                                title={input}
                                            >
                                                #{urlIndex + 1}: {displayUrl}
                                            </div>

                                            {/* Clip inputs */}
                                            <div className="flex flex-wrap gap-3">
                                                {clips.map((clip, clipIndex) => {
                                                    const startSecs = parseHmsString(clip.start);
                                                    const endSecs = parseHmsString(clip.end);

                                                    return (
                                                        <div key={clipIndex} className="flex items-center gap-2">
                                                            <TimeInput
                                                                value={startSecs}
                                                                onChange={(secs) => updateTimestamp(urlIndex, clipIndex, 'start', secs)}
                                                                onTabOut={() => {
                                                                    const el = document.getElementById(`clip-end-${urlIndex}-${clipIndex}`);
                                                                    el?.focus();
                                                                }}
                                                                themeColors={themeColors}
                                                            />
                                                            <span
                                                                className="text-lg"
                                                                style={{ color: themeColors.textMuted }}
                                                            >
                                                                →
                                                            </span>
                                                            <TimeInput
                                                                value={endSecs}
                                                                onChange={(secs) => updateTimestamp(urlIndex, clipIndex, 'end', secs)}
                                                                id={`clip-end-${urlIndex}-${clipIndex}`}
                                                                themeColors={themeColors}
                                                            />
                                                            {clipIndex < clips.length - 1 && (
                                                                <span
                                                                    className="text-lg mx-1"
                                                                    style={{ color: themeColors.textMuted }}
                                                                >
                                                                    |
                                                                </span>
                                                            )}
                                                        </div>
                                                    );
                                                })}
                                            </div>
                                            <p
                                                className="text-sm mt-2"
                                                style={{ color: themeColors.textMuted }}
                                            >
                                                Fill both start and end to add another clip
                                            </p>
                                        </div>
                                    );
                                })}
                            </div>
                        )}

                        <div className="flex justify-end mt-6">
                            <button
                                onClick={() => setShowDialog(false)}
                                className="px-4 py-2 text-sm text-white rounded-md hover:brightness-110 active:scale-95 active:brightness-90 transition-transform duration-[80ms]"
                                style={{ backgroundColor: themeColors.buttonPrimary }}
                            >
                                Done
                            </button>
                        </div>
                    </div>
                </div>
            )}
        </>
    );
}
