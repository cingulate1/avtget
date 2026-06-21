const hostEl = document.getElementById('host');
const tokenEl = document.getElementById('token');
const statusEl = document.getElementById('status');

async function load() {
  const stored = await browser.storage.local.get({
    host: 'http://127.0.0.1:47923',
    token: '',
  });
  hostEl.value = stored.host;
  tokenEl.value = stored.token;
}

function showStatus(kind, message) {
  statusEl.textContent = message;
  statusEl.className = kind;
}

document.getElementById('save').addEventListener('click', async () => {
  const host = hostEl.value.trim();
  const token = tokenEl.value.trim();
  if (!host) {
    showStatus('err', 'Host URL cannot be empty.');
    return;
  }
  if (!token) {
    showStatus('err', 'Bearer token cannot be empty.');
    return;
  }
  await browser.storage.local.set({ host, token });
  showStatus('ok', 'Saved.');
});

document.getElementById('test').addEventListener('click', async () => {
  // Save the current field values first so the health probe uses them.
  await browser.storage.local.set({
    host: hostEl.value.trim(),
    token: tokenEl.value.trim(),
  });

  showStatus('info', 'Testing\u2026');

  const resp = await browser.runtime.sendMessage({ type: 'health' });
  if (resp?.ok) {
    const version = resp.version ? ` ${resp.version}` : '';
    showStatus('ok', `OK — Avtget${version} is reachable at ${hostEl.value}.`);
  } else {
    showStatus('err', `Failed: ${resp?.error || 'unknown error'}`);
  }
});

load();
