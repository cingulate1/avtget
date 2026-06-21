const urlEl = document.getElementById('url');
const healthEl = document.getElementById('health');
const resultEl = document.getElementById('result');
const actionButtons = document.querySelectorAll('button.action');

let currentUrl = '';

async function loadCurrentTab() {
  const tabs = await browser.tabs.query({ active: true, currentWindow: true });
  currentUrl = tabs[0]?.url || '';
  urlEl.textContent = currentUrl || '(no URL available for this tab)';
}

async function checkHealth() {
  try {
    const resp = await browser.runtime.sendMessage({ type: 'health' });
    if (resp?.ok) {
      const version = resp.version ? ` ${resp.version}` : '';
      healthEl.textContent = `Avtget${version} connected`;
      healthEl.className = 'ok';
    } else {
      healthEl.textContent = `Avtget offline: ${resp?.error || 'unknown'}`;
      healthEl.className = 'err';
      setButtonsEnabled(false);
    }
  } catch (err) {
    healthEl.textContent = `Health check failed: ${err}`;
    healthEl.className = 'err';
    setButtonsEnabled(false);
  }
}

function setButtonsEnabled(enabled) {
  actionButtons.forEach((btn) => (btn.disabled = !enabled));
}

async function submit(preset) {
  if (!currentUrl) {
    resultEl.textContent = 'No URL to send.';
    resultEl.className = 'err';
    return;
  }
  resultEl.textContent = 'Sending\u2026';
  resultEl.className = '';
  resultEl.style.display = 'block';
  setButtonsEnabled(false);

  try {
    const resp = await browser.runtime.sendMessage({
      type: 'submit',
      url: currentUrl,
      preset,
    });
    if (resp?.ok) {
      resultEl.textContent = `Queued \u2713 (${preset.replace(/_/g, ' ')})`;
      resultEl.className = 'ok';
      setTimeout(() => window.close(), 650);
    } else {
      resultEl.textContent = `Failed: ${resp?.error || 'unknown'}`;
      resultEl.className = 'err';
      setButtonsEnabled(true);
    }
  } catch (err) {
    resultEl.textContent = `Error: ${err}`;
    resultEl.className = 'err';
    setButtonsEnabled(true);
  }
}

actionButtons.forEach((btn) => {
  btn.addEventListener('click', () => submit(btn.dataset.preset));
});

document.getElementById('options-link').addEventListener('click', (event) => {
  event.preventDefault();
  browser.runtime.openOptionsPage();
});

loadCurrentTab();
checkHealth();
