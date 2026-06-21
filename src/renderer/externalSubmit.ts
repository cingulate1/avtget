import type { ExternalJobRequest, ExternalJobPreset } from '@/shared/types';
import { useJobStore } from './store/jobStore';
import { useLogStore } from './store/logStore';
import { getDesktopAPI } from './desktopApi';
import { sanitizeUrl } from './utils/youtube';
import { submitInputs } from './jobDispatch';

const PRESET_LABELS: Record<ExternalJobPreset, string> = {
  archive_video: 'Archive video',
  save_audio: 'Save audio',
  save_transcript: 'Save transcript',
  summarize: 'Summarize',
};

function labelFor(preset: string): string {
  return (PRESET_LABELS as Record<string, string>)[preset] ?? preset;
}

// A Firefox intake is "just an input". It mirrors a manual submission exactly:
//   1. set the preset's checkboxes (and leave them set), and
//   2. drop the URL into the uppermost empty input field, then
//   3. fire the same GO signal the Go button does.
// Everything downstream — dedup against existing jobs, per-job config freeze,
// queueing, one-process-per-input dispatch — is identical to a manual Go. A
// busy app just means the new job queues and auto-runs in turn.
//
// Bad UX is deliberate for now: the checkboxes visibly change and submitted
// inputs are not cleared. The robust processing schema comes first; UX polish
// is a later version.
export function handleExternalJobRequest(request: ExternalJobRequest): void {
  const jobStore = useJobStore.getState();
  const logStore = useLogStore.getState();
  const label = labelFor(request.preset);

  // 1. Make sure the preset's boxes are CHECKED — additively. The plugin only
  //    ever turns boxes on; it never unchecks the user's existing selections.
  //    (So a "save audio" intake while Video is already checked archives video
  //    too — that's the intended only-check behavior.) setModes mirrors the
  //    result to config.ini (so the freeze captures it) and applies the
  //    transcript→summarize gate; the merged modes also ride the frozen payload.
  const current = jobStore.currentModes;
  jobStore.setModes({
    video: current.video || request.modes.video,
    audio: current.audio || request.modes.audio,
    transcript: current.transcript || request.modes.transcript,
    summarize: current.summarize || request.modes.summarize,
  });

  // 2. Write the URL into the uppermost empty input slot, preserving anything
  //    already typed, and keep a single trailing empty field per convention.
  const sanitized = sanitizeUrl(request.url);
  const next = [...useJobStore.getState().inputs];
  const emptyIndex = next.findIndex((value) => value.trim() === '');
  if (emptyIndex >= 0) {
    next[emptyIndex] = sanitized;
  } else {
    next.push(sanitized);
  }
  if (next.length === 0 || next[next.length - 1].trim() !== '') {
    next.push('');
  }
  jobStore.setInputs(next);

  const msg = `Received from Firefox (${label}): ${request.url}`;
  logStore.addLog(msg);
  void getDesktopAPI().logMessage(msg);

  // 3. Fire the unified GO. A URL already represented by a job is a harmless
  //    no-op (dedup); otherwise it becomes a new job that auto-runs in turn.
  void submitInputs();
}
