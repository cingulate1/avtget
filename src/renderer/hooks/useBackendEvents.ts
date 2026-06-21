import { useEffect, useRef } from 'react';
import { useJobStore } from '../store/jobStore';
import { useLogStore } from '../store/logStore';
import { sanitizeUrl } from '../utils/youtube';
import { getDesktopAPI } from '../desktopApi';
import { startExternalJobNow } from '../externalSubmit';
import type { BackendEvent, ArtifactStatusEvent, StatusChangeEvent, StageCountEvent, ProgressEvent, JobFinishedEvent, JobErrorEvent, LogEvent } from '@/shared/types';

// Helper to format stage status - skips "getting" prefix for stages that already have a verb
function formatStageStatus(stageName: string, current: number, total: number): string {
  // Stages that already start with a verb don't need "getting" prefix
  if (stageName.startsWith('cleaning ')) {
    return `${stageName} (${current}/${total})`;
  }
  return `getting ${stageName} (${current}/${total})`;
}

export function useBackendEvents() {
  const desktopAPI = getDesktopAPI();
  const addJob = useJobStore((s) => s.addJob);
  const updateJobStatus = useJobStore((s) => s.updateJobStatus);
  const updateArtifactStatus = useJobStore((s) => s.updateArtifactStatus);
  const setFilestemOverride = useJobStore((s) => s.setFilestemOverride);
  const setStatusTotal = useJobStore((s) => s.setStatusTotal);
  const setRunning = useJobStore((s) => s.setRunning);
  const clearInputsForCompletedJobs = useJobStore((s) => s.clearInputsForCompletedJobs);

  const addLog = useLogStore((s) => s.addLog);
  const setStatusLine = useLogStore((s) => s.setStatusLine);
  const setHasJobError = useLogStore((s) => s.setHasJobError);

  const stageCountRef = useRef<{ current: number; total: number; stageName: string }>({ current: 0, total: 0, stageName: '' });

  useEffect(() => {
    const unsubscribe = desktopAPI.onBackendEvent((event: BackendEvent) => {
      switch (event.type) {
        case 'log': {
          const logEvent = event as LogEvent;
          const isError = logEvent.message.toLowerCase().includes('error') ||
            logEvent.message.toLowerCase().includes('failed');
          addLog(logEvent.message, isError);
          break;
        }

        case 'status_change': {
          const statusEvent = event as StatusChangeEvent;
          const itemId = sanitizeUrl(statusEvent.item_id);
          addJob(itemId);
          updateJobStatus(itemId, statusEvent.status);
          break;
        }

        case 'artifact_status': {
          const artifactEvent = event as ArtifactStatusEvent;
          const itemId = sanitizeUrl(artifactEvent.item_id);
          if (artifactEvent.artifact === '__filestem__') {
            addJob(itemId);
            setFilestemOverride(itemId, artifactEvent.status);
          } else {
            addJob(itemId);
            updateArtifactStatus(itemId, artifactEvent.artifact, artifactEvent.status);

            if (artifactEvent.status === 'running') {
              const { current, total, stageName } = stageCountRef.current;
              const shownTotal = total > 0 ? total : 1;
              const shownCurrent = current > 0 ? current : 1;
              // Use stage name if available (preserves model info like "transcript (whisper-large-v3)")
              const displayName = stageName || artifactEvent.artifact;
              setStatusLine(formatStageStatus(displayName, shownCurrent, shownTotal));
            }
          }
          break;
        }

        case 'stage_count': {
          const stageEvent = event as StageCountEvent;
          stageCountRef.current = { current: stageEvent.current, total: stageEvent.total, stageName: stageEvent.stage_name };
          setStatusTotal(stageEvent.total);
          setStatusLine(formatStageStatus(stageEvent.stage_name, stageEvent.current, stageEvent.total));
          break;
        }

        case 'progress': {
          // Update stage name when progress event has a different stage (e.g., audio -> transcript)
          const progressEvent = event as ProgressEvent;
          const { current, total } = stageCountRef.current;
          const shownTotal = total > 0 ? total : 1;
          const shownCurrent = current > 0 ? current : 1;
          // Update the stored stage name and status line
          stageCountRef.current.stageName = progressEvent.stage;
          setStatusLine(formatStageStatus(progressEvent.stage, shownCurrent, shownTotal));
          break;
        }

        case 'job_finished': {
          const finishedEvent = event as JobFinishedEvent;
          setRunning(false);
          addLog(finishedEvent.summary);
          setStatusLine('');
          // Clear input URLs for successfully completed jobs
          clearInputsForCompletedJobs();
          // Remove placeholder jobs that were never processed by the backend
          // (e.g., playlist/channel URLs that were expanded into individual video jobs)
          const { jobs, removeJob: remove } = useJobStore.getState();
          for (const [id, job] of jobs) {
            if (!job.displayName && job.status === 'queued') {
              remove(id);
            }
          }
          break;
        }

        case 'job_error': {
          const errorEvent = event as JobErrorEvent;
          setRunning(false);
          setHasJobError(true);  // Mark that an actual job error occurred
          addLog(`Error: ${errorEvent.error}`, true);
          setStatusLine('');
          break;
        }

        case 'backend_exited': {
          // The backend process has fully exited and the shell's process
          // slot is clear — only now is it safe to start the next queued
          // Firefox-extension intake. (`job_finished` arrives before the
          // process exits; draining there raced the exit monitor and a
          // popped intake could be lost to "Backend already running".)
          // One at a time — the next backend_exited drains the next entry.
          // Error exits emit this too, so the queue no longer stalls when a
          // batch dies without a job_finished.
          const nextExternal = useJobStore.getState().drainNextExternalJob();
          if (nextExternal) {
            void startExternalJobNow(nextExternal);
          }
          break;
        }
      }
    });

    return unsubscribe;
  }, [addJob, updateJobStatus, updateArtifactStatus, setFilestemOverride, setStatusTotal, setRunning, addLog, setStatusLine, setHasJobError, clearInputsForCompletedJobs]);
}
