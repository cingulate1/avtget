import type { Settings, BackendEvent, ExternalJobRequest } from '../shared/types';

export interface DesktopAPI {
  // Job control. startJob resolves once the backend process has been spawned
  // and rejects if it could not be (e.g. the previous backend's process slot
  // hasn't cleared yet) — callers must handle the rejection.
  startJob(config: any): Promise<void>;
  cancelJob(): void;
  onBackendEvent(callback: (event: BackendEvent) => void): () => void;
  onExternalJobRequest(callback: (request: ExternalJobRequest) => void): () => void;

  // Configuration
  getConfig(): Promise<Settings>;
  saveConfig(settings: Settings): Promise<void>;
  // Live working-state mirror: persist the five main-window checkboxes to
  // config.ini immediately on every change (unidirectional GUI -> file). The
  // GUI seeds these from the default_* keys once on startup and never reads
  // them back, so external edits to config.ini are not reflected while running.
  setLiveModes(modes: {
    video: boolean;
    audio: boolean;
    transcript: boolean;
    summarize: boolean;
    verbose: boolean;
  }): Promise<void>;
  // Freeze the current config.ini into a per-job snapshot and resolve with its
  // absolute path. Called at GO time (once per submitted input); the path rides
  // that job's JobConfig so the spawned backend reads the frozen copy via
  // AVTGET_CONFIG_PATH and is immune to later config edits.
  freezeConfig(): Promise<string>;
  // Dialogs
  showOpenDialog(options: any): Promise<string | null>;
  showMessageBox(options: any): Promise<number>;
  showSaveDialog(options: any): Promise<{ canceled: boolean; filePath?: string }>;

  // Index operations
  getIndexEntry(videoId: string): Promise<any | null>;
  reloadIndex(): Promise<void>;

  // File operations
  readTextFile(filePath: string): Promise<string | null>;
  writeTextFile(filePath: string, content: string): Promise<boolean>;

  // Reveal output files
  revealArtifact(artifact: 'video' | 'audio' | 'transcript' | 'summary', filestem: string): Promise<boolean>;

  // Open output files with default app
  openArtifact(artifact: 'video' | 'audio' | 'transcript' | 'summary', filestem: string): Promise<boolean>;

  // Append a line to avtget_debug.log without re-broadcasting it to the
  // frontend. Used for messages the frontend generates directly (e.g., Firefox
  // intakes) so the debug log mirrors the verbose UI log 1:1.
  logMessage(message: string): Promise<void>;

  // Expand a YouTube playlist URL into its component video URLs via yt-dlp.
  // Resolves with canonical `https://www.youtube.com/watch?v=ID` strings.
  // Rejects with a descriptive error if yt-dlp fails.
  expandPlaylist(url: string): Promise<string[]>;

  // Drag and drop — subscribe to OS-level file drops on the window.
  // onDrop receives absolute filesystem paths; onDragStateChange fires true
  // when a drag enters the window and false when it leaves or drops.
  // Returns an unlisten function to clean up the subscription.
  onFileDrop(
    onDrop: (paths: string[]) => void,
    onDragStateChange: (isDragging: boolean) => void,
  ): () => void;

  // Zoom control
  getZoomLevel(): number;
  setZoomLevel(level: number): void;

  // Menu events
  onSavePresetRequest(callback: () => void): () => void;
}

declare global {
  interface Window {
    desktopAPI?: DesktopAPI;
  }
}
