// Avtget Sender — background script.
//
// Handles the context-menu entries and message routing between the popup /
// options pages and the local Avtget HTTP intake server.

const DEFAULT_CONFIG = {
  host: 'http://127.0.0.1:47923',
  token: '',
};

const PRESET_LABELS = {
  archive_video: 'Archive video',
  save_audio: 'Save audio',
  save_transcript: 'Save transcript',
  summarize: 'Summarize',
};

const MENU_ITEMS = [
  { id: 'avtget-archive', title: 'Avtget: Archive video', preset: 'archive_video' },
  { id: 'avtget-audio', title: 'Avtget: Save audio', preset: 'save_audio' },
  { id: 'avtget-transcript', title: 'Avtget: Save transcript', preset: 'save_transcript' },
  { id: 'avtget-summarize', title: 'Avtget: Summarize', preset: 'summarize' },
];

async function getConfig() {
  const stored = await browser.storage.local.get(DEFAULT_CONFIG);
  return { ...DEFAULT_CONFIG, ...stored };
}

function normalizeHost(host) {
  return (host || '').trim().replace(/\/+$/, '');
}

async function submitJob(url, preset) {
  const config = await getConfig();
  if (!config.token) {
    throw new Error(
      "No Avtget bearer token configured. Open the extension options page and paste the token from Avtget's config.ini."
    );
  }
  const endpoint = `${normalizeHost(config.host)}/jobs`;
  const response = await fetch(endpoint, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${config.token}`,
    },
    body: JSON.stringify({ url, preset }),
  });
  if (!response.ok) {
    const body = await response.text();
    throw new Error(`Avtget returned ${response.status}: ${body || response.statusText}`);
  }
  return response.json();
}

async function probeHealth() {
  const config = await getConfig();
  const endpoint = `${normalizeHost(config.host)}/health`;
  const response = await fetch(endpoint);
  if (!response.ok) {
    throw new Error(`HTTP ${response.status}`);
  }
  return response.json();
}

async function notify(title, message) {
  try {
    await browser.notifications.create({
      type: 'basic',
      iconUrl: browser.runtime.getURL('icons/icon-48.png'),
      title,
      message,
    });
  } catch (_err) {
    // Notification failures are non-fatal — the popup result UI covers it.
  }
}

async function setupContextMenus() {
  try {
    await browser.contextMenus.removeAll();
  } catch (_err) {}
  for (const item of MENU_ITEMS) {
    browser.contextMenus.create({
      id: item.id,
      title: item.title,
      contexts: ['page', 'link', 'video', 'audio'],
    });
  }
}

browser.runtime.onInstalled.addListener(setupContextMenus);
browser.runtime.onStartup.addListener(setupContextMenus);

browser.contextMenus.onClicked.addListener(async (info, tab) => {
  const menuItem = MENU_ITEMS.find((item) => item.id === info.menuItemId);
  if (!menuItem) return;

  // Prefer explicitly clicked targets over the page URL.
  const url = info.linkUrl || info.srcUrl || info.pageUrl || tab?.url;
  if (!url) {
    await notify('Avtget', 'No URL detected for this context.');
    return;
  }

  try {
    await submitJob(url, menuItem.preset);
    await notify('Avtget queued', `${PRESET_LABELS[menuItem.preset]}: ${url}`);
  } catch (err) {
    await notify('Avtget error', String(err?.message || err));
  }
});

// Popup / options page message handler. Returning a Promise keeps the message
// channel alive across the await boundaries in Firefox MV3.
browser.runtime.onMessage.addListener((message) => {
  if (message?.type === 'submit') {
    return submitJob(message.url, message.preset)
      .then((result) => ({ ok: true, result }))
      .catch((err) => ({ ok: false, error: String(err?.message || err) }));
  }
  if (message?.type === 'health') {
    return probeHealth()
      .then((data) => ({ ok: true, ...data }))
      .catch((err) => ({ ok: false, error: String(err?.message || err) }));
  }
  return Promise.resolve({ ok: false, error: 'unknown message' });
});
