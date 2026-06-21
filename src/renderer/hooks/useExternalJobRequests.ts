import { useEffect } from 'react';
import { getDesktopAPI } from '../desktopApi';
import { handleExternalJobRequest } from '../externalSubmit';

// Listens for job requests emitted by the local HTTP intake server in the
// Tauri shell (fed by the Firefox extension) and routes them through the
// normal submission flow.
export function useExternalJobRequests(): void {
  useEffect(() => {
    const desktopAPI = getDesktopAPI();
    const unsubscribe = desktopAPI.onExternalJobRequest((request) => {
      handleExternalJobRequest(request);
    });
    return unsubscribe;
  }, []);
}
