import { useRef, useEffect, useState } from 'react';
import { useLogStore } from '../store/logStore';
import { useJobStore } from '../store/jobStore';
import { useThemeStore, themes } from '../store/themeStore';
import CopyIcon from '../../../assets/copy.svg?raw';

export function LogPanel() {
  const logs = useLogStore((s) => s.logs);
  const statusLine = useLogStore((s) => s.statusLine);
  const hasJobError = useLogStore((s) => s.hasJobError);
  const verboseMode = useJobStore((s) => s.verboseMode);
  const currentTheme = useThemeStore((s) => s.theme);
  const themeColors = themes[currentTheme];
  const scrollRef = useRef<HTMLDivElement>(null);
  const [logHeight, setLogHeight] = useState(286); // Default height in px
  const [isResizing, setIsResizing] = useState(false);

  const handleCopyLogs = () => {
    const logText = logs.map(log => log.message).join('\n');
    navigator.clipboard.writeText(logText);
  };

  // Auto-scroll to bottom when visible logs change
  useEffect(() => {
    if (scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, [logs, verboseMode]);

  // Handle resize
  const handleMouseDown = (e: React.MouseEvent) => {
    e.preventDefault();
    setIsResizing(true);
  };

  useEffect(() => {
    if (!isResizing) return;

    const handleMouseMove = (e: MouseEvent) => {
      // Calculate new height based on mouse position relative to window bottom
      const windowHeight = window.innerHeight;
      const newHeight = windowHeight - e.clientY - 40; // 40px for padding
      setLogHeight(Math.max(64, newHeight)); // Min 64px, no max limit
    };

    const handleMouseUp = () => {
      setIsResizing(false);
    };

    document.addEventListener('mousemove', handleMouseMove);
    document.addEventListener('mouseup', handleMouseUp);

    return () => {
      document.removeEventListener('mousemove', handleMouseMove);
      document.removeEventListener('mouseup', handleMouseUp);
    };
  }, [isResizing]);

  // Filter logs based on verbose mode:
  // verbose on = show all, verbose off = show only errors and job_finished summaries
  const visibleLogs = verboseMode
    ? logs
    : logs.filter((log) => log.isError);

  // In non-verbose mode with no errors to show, display simple inline status
  if (!verboseMode && visibleLogs.length === 0) {
    return (
      <div
        className="px-4 py-1 text-sm font-medium transition-all duration-[400ms]"
        style={{ color: themeColors.text }}
      >
        {hasJobError ? (
          <span className="text-red-500">Error</span>
        ) : statusLine ? (
          <span style={{ color: themeColors.buttonPrimary }}>{statusLine}</span>
        ) : (
          <span style={{ color: themeColors.textMuted }}>Ready</span>
        )}
      </div>
    );
  }

  // Full log box with card wrapper and resize handle
  return (
    <div
      className="rounded-lg shadow p-4 transition-all duration-[400ms]"
      style={{ backgroundColor: themeColors.cardBg }}
    >
      {/* Resize handle at top */}
      <div
        className="h-2 cursor-ns-resize"
        onMouseDown={handleMouseDown}
      />
      <div
        className="rounded-md flex flex-col transition-all duration-[400ms]"
        style={{
          border: `1px solid ${themeColors.border}`,
          height: `${logHeight}px`,
        }}
      >
        <div
          className="px-3 py-1 text-xs font-semibold flex-shrink-0 transition-all duration-[400ms] flex items-center justify-between"
          style={{
            backgroundColor: themeColors.headerBg,
            color: themeColors.text,
            borderBottom: `1px solid ${themeColors.border}`,
          }}
        >
          <span>Log</span>
          <button
            onClick={handleCopyLogs}
            className="p-1 rounded hover:opacity-75 transition-opacity"
            title="Copy log to clipboard"
            style={{ color: themeColors.text }}
          >
            <span
              className="w-4 h-4 block"
              dangerouslySetInnerHTML={{ __html: CopyIcon }}
            />
          </button>
        </div>
        <div ref={scrollRef} className="flex-1 overflow-auto px-2 py-1">
          <div className="font-mono text-xs space-y-0.5">
            {visibleLogs.map((log, i) => (
              <div
                key={i}
                className="transition-all duration-[400ms]"
                style={{ color: log.isError ? '#ef4444' : themeColors.text }}
              >
                {log.message}
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}
