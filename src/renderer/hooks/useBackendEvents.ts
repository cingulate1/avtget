import { useEffect, useRef } from 'react';
import { useJobStore } from '../store/jobStore';
import { useLogStore } from '../store/logStore';
import { sanitizeUrl } from '../utils/youtube';
import { getDesktopAPI } from '../desktopApi';
import { dispatchNext } from '../jobDispatch';
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
          // isRunning is owned by `backend_exited` (fired once the process slot
          // actually clears). job_finished arrives while the process is still
          // alive, so we deliberately don't touch isRunning here.
          addLog(finishedEvent.summary);
          setStatusLine('');
          // Clear input URLs for successfully completed jobs
          clearInputsForCompletedJobs();
          // Remove placeholder jobs that the backend never processed (e.g. a
          // playlist/channel URL expanded into individual video jobs). Skip
          // anything still in the job queue — those are legitimately queued
          // future work, not stale placeholders, and must not be deleted.
          const { jobs, jobQueue, removeJob: remove } = useJobStore.getState();
          const queuedIds = new Set(jobQueue.map((unit) => unit.itemId));
          for (const [id, job] of jobs) {
            if (!job.displayName && job.status === 'queued' && !queuedIds.has(id)) {
              remove(id);
            }
          }
          break;
        }

        case 'job_error': {
          const errorEvent = event as JobErrorEvent;
          // isRunning is cleared by backend_exited, which always follows an
          // error exit — leaving it owned by one place keeps the dispatch in
          // sync with the real process slot.
          setHasJobError(true);  // Mark that an actual job error occurred
          addLog(`Error: ${errorEvent.error}`, true);
          setStatusLine('');
          break;
        }

        case 'backend_exited': {
          // The backend process has fully exited and the shell's process slot
          // is clear — only now is it safe to start the next queued job.
          // (`job_finished` arrives before the process exits; dispatching there
          // raced the exit monitor and a unit could be lost to "Backend already
          // running".) Error exits emit this too, so the queue never stalls on
          // a job that dies without a job_finished. One at a time — the next
          // backend_exited dispatches the next unit.
          setRunning(false);
          dispatchNext();
          break;
        }
      }
    });

    return unsubscribe;
  }, [addJob, updateJobStatus, updateArtifactStatus, setFilestemOverride, setStatusTotal, setRunning, addLog, setStatusLine, setHasJobError, clearInputsForCompletedJobs]);
}
