import type { DesktopAPI } from './window';
import { getTauriDesktopAPI, isTauriRuntime } from './desktopApiTauri';

export function getDesktopAPI(): DesktopAPI {
  if (window.desktopAPI) {
    return window.desktopAPI;
  }

  if (isTauriRuntime()) {
    const tauriApi = getTauriDesktopAPI();
    window.desktopAPI = tauriApi;
    return tauriApi;
  }

  throw new Error('Desktop API bridge is not available');
}
