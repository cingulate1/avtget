import { useState, useEffect, useRef } from 'react';
import { useJobStore } from '../store/jobStore';
import { useThemeStore, themes } from '../store/themeStore';
import { getStatusEmoji, getRowBackgroundColor } from '../utils/status';
import { getDesktopAPI } from '../desktopApi';
import { isChannelUrl } from '../utils/youtube';
import type { JobItem } from '@/shared/types';

interface ContextMenu {
  x: number;
  y: number;
  itemId: string;
}

export function JobTable() {
  const desktopAPI = getDesktopAPI();
  const jobs = useJobStore((s) => s.jobs);
  const clearCompletedJobs = useJobStore((s) => s.clearCompletedJobs);
  const clearAllJobs = useJobStore((s) => s.clearAllJobs);
  const removeJob = useJobStore((s) => s.removeJob);
  const currentTheme = useThemeStore((s) => s.theme);
  const themeColors = themes[currentTheme];
  const [contextMenu, setContextMenu] = useState<ContextMenu | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  // Close context menu on click anywhere or Escape
  useEffect(() => {
    if (!contextMenu) return;
    const close = () => setContextMenu(null);
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') close(); };
    document.addEventListener('mousedown', close);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', close);
      document.removeEventListener('keydown', onKey);
    };
  }, [contextMenu]);

  const handleRowContextMenu = (e: React.MouseEvent, job: JobItem) => {
    e.preventDefault();
    e.stopPropagation();
    setContextMenu({ x: e.clientX, y: e.clientY, itemId: job.itemId });
  };

  const handleCopyUrl = () => {
    if (contextMenu) {
      navigator.clipboard.writeText(contextMenu.itemId);
      setContextMenu(null);
    }
  };

  const handleRemoveJob = (job: JobItem) => {
    if (job.status === 'running') {
      desktopAPI.cancelJob();
    }
    removeJob(job.itemId);
  };

  // Total cull: cancel the running backend (if any), then wipe the queue and
  // every row. No "Are you sure?" — deliberate, per the app's philosophy. Also
  // the only way to clear a hidden channel placeholder left stuck after a Stop
  // (it has no visible ⛔), short of restarting the app.
  const handleClearAll = () => {
    desktopAPI.cancelJob();
    clearAllJobs();
  };

  // A channel URL is a container, not a downloadable item: at GO it's reserved
  // as a job (the synchronous dedup anchor that keeps button-mashing a no-op),
  // but the backend expands it into per-video jobs and never emits events keyed
  // by the channel URL itself — so it never gets a displayName and would render
  // as a permanent "Loading..." orphan until job_finished sweeps it up. Hide it.
  const allJobs = Array.from(jobs.values());
  const jobsArray = allJobs.filter(
    (job) => !(isChannelUrl(job.itemId) && !job.displayName),
  );

  // Empty-state keys off the RAW map, not the filtered rows. A lone hidden
  // channel placeholder (e.g. left behind when a scrape is Stopped before any
  // video is found) has no visible row, but we must keep the header — and its
  // Clear All button — reachable so the user can cull that otherwise-stuck
  // dedup anchor without restarting the app.
  if (allJobs.length === 0) {
    return (
      <div
        className="flex-1 rounded-md p-4 text-center transition-all duration-[400ms]"
        style={{
          border: `1px solid ${themeColors.border}`,
          color: themeColors.textMuted,
        }}
      >
        No jobs queued
      </div>
    );
  }

  return (
    <div className="flex flex-col h-full">
      <div className="flex justify-between items-center mb-2">
        <h3
          className="text-sm font-semibold transition-all duration-[400ms]"
          style={{ color: themeColors.text }}
        >
          Jobs
        </h3>
        <div className="flex gap-2">
          <button
            onClick={handleClearAll}
            className="px-3 py-1 text-sm text-white rounded transition-transform duration-[80ms] hover:brightness-110 active:scale-95 active:brightness-90"
            style={{ backgroundColor: themeColors.buttonSecondary }}
          >
            Clear All
          </button>
          <button
            onClick={clearCompletedJobs}
            className="px-3 py-1 text-sm text-white rounded transition-transform duration-[80ms] hover:brightness-110 active:scale-95 active:brightness-90"
            style={{ backgroundColor: themeColors.buttonPrimary }}
          >
            Clear Completed
          </button>
        </div>
      </div>
      <div
        className="flex-1 rounded-md overflow-auto transition-all duration-[400ms]"
        style={{ border: `1px solid ${themeColors.border}` }}
      >
        <table className="w-full text-sm">
          <thead
            className="sticky top-0 transition-all duration-[400ms]"
            style={{ backgroundColor: themeColors.headerBg }}
          >
            <tr>
              <th className="px-1 py-2 w-8"></th>
              <th
                className="text-left px-3 py-2 font-semibold transition-all duration-[400ms]"
                style={{ color: themeColors.text }}
              >
                Item
              </th>
              <th
                className="text-center px-3 py-2 w-12 font-semibold transition-all duration-[400ms]"
                style={{ color: themeColors.text }}
              >
                V
              </th>
              <th
                className="text-center px-3 py-2 w-12 font-semibold transition-all duration-[400ms]"
                style={{ color: themeColors.text }}
              >
                A
              </th>
              <th
                className="text-center px-3 py-2 w-12 font-semibold transition-all duration-[400ms]"
                style={{ color: themeColors.text }}
              >
                T
              </th>
              <th
                className="text-center px-3 py-2 w-12 font-semibold transition-all duration-[400ms]"
                style={{ color: themeColors.text }}
              >
                S
              </th>
            </tr>
          </thead>
          <tbody>
            {jobsArray.map((job) => {
              const rowBg = getRowBackgroundColor(job.status);
              // Only show filestem (filename) - never show URLs
              const displayText = job.displayName || 'Loading...';
              const renderArtifactStatus = (artifact: 'video' | 'audio' | 'transcript' | 'summary') => {
                const status = job.artifacts[artifact];
                const emoji = getStatusEmoji(status);
                // Allow interaction for completed or warning status (warning means raw transcript was saved)
                const canInteract = (status === 'completed' || status === 'warning') && Boolean(job.displayName);

                if (!canInteract) {
                  return <span>{emoji}</span>;
                }

                // Use a closure to track click timeout for this specific button
                let clickTimeout: ReturnType<typeof setTimeout> | null = null;

                const handleClick = (e: React.MouseEvent) => {
                  e.preventDefault();
                  e.stopPropagation();

                  // If there's already a pending click, this is part of a double-click
                  if (clickTimeout) {
                    return;
                  }

                  // Set timeout for single-click action
                  clickTimeout = setTimeout(() => {
                    clickTimeout = null;
                    void desktopAPI.revealArtifact(artifact, job.displayName);
                  }, 250);
                };

                const handleDoubleClick = (e: React.MouseEvent) => {
                  e.preventDefault();
                  e.stopPropagation();

                  // Clear the single-click timeout
                  if (clickTimeout) {
                    clearTimeout(clickTimeout);
                    clickTimeout = null;
                  }

                  // Execute double-click action
                  void desktopAPI.openArtifact(artifact, job.displayName);
                };

                return (
                  <button
                    type="button"
                    className="bg-transparent border-0 p-0 m-0 cursor-pointer hover:opacity-80 select-none focus:outline-none"
                    title="Click: reveal in folder | Double-click: open file"
                    aria-label={`Click to reveal ${artifact}, double-click to open`}
                    onClick={handleClick}
                    onDoubleClick={handleDoubleClick}
                  >
                    {emoji}
                  </button>
                );
              };
              return (
                <tr key={job.itemId} className={rowBg} onContextMenu={(e) => handleRowContextMenu(e, job)}>
                  <td className="text-center px-1 py-2 w-8">
                    {(job.status === 'queued' || job.status === 'running') && (
                      <button
                        type="button"
                        className="bg-transparent border-0 p-0 m-0 cursor-pointer hover:opacity-70 select-none focus:outline-none"
                        title={job.status === 'running' ? 'Cancel job' : 'Remove from queue'}
                        onClick={() => handleRemoveJob(job)}
                      >
                        ⛔
                      </button>
                    )}
                  </td>
                  <td
                    className="px-3 py-2 transition-all duration-[400ms]"
                    style={{ color: job.displayName ? themeColors.text : themeColors.textMuted }}
                  >
                    {displayText}
                  </td>
                  <td className="text-center px-3 py-2">
                    {renderArtifactStatus('video')}
                  </td>
                  <td className="text-center px-3 py-2">
                    {renderArtifactStatus('audio')}
                  </td>
                  <td className="text-center px-3 py-2">
                    {renderArtifactStatus('transcript')}
                  </td>
                  <td className="text-center px-3 py-2">
                    {renderArtifactStatus('summary')}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>

      {/* Context menu */}
      {contextMenu && (
        <div
          ref={menuRef}
          className="fixed z-50 rounded shadow-lg py-1 text-sm"
          style={{
            left: contextMenu.x,
            top: contextMenu.y,
            backgroundColor: themeColors.cardBg,
            border: `1px solid ${themeColors.border}`,
            color: themeColors.text,
          }}
          onMouseDown={(e) => e.stopPropagation()}
        >
          <button
            type="button"
            className="w-full text-left px-4 py-1.5 hover:brightness-125 cursor-pointer"
            style={{ backgroundColor: themeColors.headerBg, color: themeColors.text }}
            onClick={handleCopyUrl}
          >
            Copy URL
          </button>
        </div>
      )}
    </div>
  );
}
