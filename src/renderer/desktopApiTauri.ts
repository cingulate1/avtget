import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import type { BackendEvent, ExternalJobRequest, Settings } from '@/shared/types';
import type { DesktopAPI } from './window';

let zoomLevel = 0;

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function toMessageText(options: any): string {
  return [options?.message, options?.detail].filter(Boolean).join('\n\n');
}

function messageBoxResultFromConfirm(options: any, confirmed: boolean): number {
  const buttons: string[] = Array.isArray(options?.buttons)
    ? options.buttons.map((button: unknown) => String(button))
    : ['OK'];

  if (buttons.length < 2) {
    return Number(options?.defaultId ?? 0);
  }

  const first = buttons[0].toLowerCase();
  if (first.includes('cancel')) {
    return confirmed ? 1 : 0;
  }

  return confirmed ? 0 : 1;
}

const tauriDesktopApi: DesktopAPI = {
  startJob(config: any): Promise<void> {
    return invoke('start_job', { config });
  },

  cancelJob(): void {
    void invoke('cancel_job');
  },

  onBackendEvent(callback: (event: BackendEvent) => void): () => void {
    let unlistenFn: (() => void) | null = null;
    let disposed = false;

    void listen<BackendEvent>('backend-event', (event) => {
      callback(event.payload);
    }).then((unlisten) => {
      if (disposed) {
        unlisten();
      } else {
        unlistenFn = unlisten;
      }
    });

    return () => {
      disposed = true;
      if (unlistenFn) {
        unlistenFn();
      }
    };
  },

  onExternalJobRequest(callback: (request: ExternalJobRequest) => void): () => void {
    let unlistenFn: (() => void) | null = null;
    let disposed = false;

    void listen<ExternalJobRequest>('external-job-request', (event) => {
      callback(event.payload);
    }).then((unlisten) => {
      if (disposed) {
        unlisten();
      } else {
        unlistenFn = unlisten;
      }
    });

    return () => {
      disposed = true;
      if (unlistenFn) {
        unlistenFn();
      }
    };
  },

  getConfig(): Promise<Settings> {
    return invoke<Settings>('get_config');
  },

  saveConfig(settings: Settings): Promise<void> {
    return invoke('save_config', { settings }).then(() => undefined);
  },

  showOpenDialog(options: any): Promise<string | null> {
    return invoke<string | null>('show_open_dialog', { options });
  },

  async showMessageBox(options: any): Promise<number> {
    const message = toMessageText(options) || options?.title || 'Confirm';
    const buttons: string[] = Array.isArray(options?.buttons)
      ? options.buttons.map((button: unknown) => String(button))
      : ['OK'];

    if (buttons.length >= 2) {
      const confirmed = window.confirm(message);
      return messageBoxResultFromConfirm(options, confirmed);
    }

    window.alert(message);
    return Number(options?.defaultId ?? 0);
  },

  showSaveDialog(options: any): Promise<{ canceled: boolean; filePath?: string }> {
    return invoke<{ canceled: boolean; filePath?: string }>('show_save_dialog', { options });
  },

  getIndexEntry(videoId: string): Promise<any | null> {
    return invoke<any | null>('get_index_entry', { video_id: videoId });
  },

  reloadIndex(): Promise<void> {
    return invoke('reload_index').then(() => undefined);
  },

  readTextFile(filePath: string): Promise<string | null> {
    return invoke<string | null>('read_text_file', { filePath });
  },

  writeTextFile(filePath: string, content: string): Promise<boolean> {
    return invoke<boolean>('write_text_file', { filePath, content });
  },

  revealArtifact(
    artifact: 'video' | 'audio' | 'transcript' | 'summary',
    filestem: string
  ): Promise<boolean> {
    return invoke<boolean>('reveal_artifact', { args: { artifact, filestem } });
  },

  openArtifact(
    artifact: 'video' | 'audio' | 'transcript' | 'summary',
    filestem: string
  ): Promise<boolean> {
    return invoke<boolean>('open_artifact', { args: { artifact, filestem } });
  },

  logMessage(message: string): Promise<void> {
    return invoke('log_message', { message }).then(() => undefined);
  },

  expandPlaylist(url: string): Promise<string[]> {
    return invoke<string[]>('expand_playlist', { url });
  },

  onFileDrop(
    onDrop: (paths: string[]) => void,
    onDragStateChange: (isDragging: boolean) => void,
  ): () => void {
    let unlistenFn: (() => void) | null = null;
    let disposed = false;

    void getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type === 'over') {
        onDragStateChange(true);
      } else if (event.payload.type === 'drop') {
        onDragStateChange(false);
        if (event.payload.paths.length > 0) {
          onDrop(event.payload.paths);
        }
      } else {
        // 'cancel' — drag left the window without dropping
        onDragStateChange(false);
      }
    }).then((unlisten) => {
      if (disposed) {
        unlisten();
      } else {
        unlistenFn = unlisten;
      }
    });

    return () => {
      disposed = true;
      if (unlistenFn) {
        unlistenFn();
      }
    };
  },

  getZoomLevel(): number {
    return zoomLevel;
  },

  setZoomLevel(level: number): void {
    zoomLevel = level;
    const scale = clamp(1 + level * 0.1, 0.5, 3);
    document.body.style.zoom = String(scale);
  },

  onSavePresetRequest(callback: () => void): () => void {
    const handler = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 's') {
        event.preventDefault();
        callback();
      }
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  },
};

export function isTauriRuntime(): boolean {
  return (
    typeof window !== 'undefined' &&
    ('__TAURI_INTERNALS__' in window || '__TAURI__' in window)
  );
}

export function getTauriDesktopAPI(): DesktopAPI {
  return tauriDesktopApi;
}
