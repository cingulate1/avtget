export function isChannelUrl(url: string): boolean {
  if (!url) return false;
  return url.includes('/@') ||
         url.includes('/c/') ||
         url.includes('/channel/') ||
         url.includes('/user/');
}

// Strict YouTube playlist URL match. Only `(https?://)?(www\.)?youtube.com/playlist?list=…`
// qualifies — a `watch?v=ID&list=…` video-in-playlist URL is not a playlist input.
export function isPlaylistUrl(url: string): boolean {
  if (!url) return false;
  return /^(?:https?:\/\/)?(?:www\.)?youtube\.com\/playlist\?list=/i.test(url.trim());
}

export function isPodcastUrl(url: string): boolean {
  if (!url) return false;
  const urlLower = url.toLowerCase();

  // Overcast pattern
  if (/overcast\.fm\/itunes\d+/.test(urlLower)) return true;

  // RSS patterns
  if (urlLower.endsWith('.xml') || urlLower.endsWith('.rss')) return true;
  if (/\/(feed|rss)\//i.test(urlLower)) return true;

  return false;
}

export function extractVideoId(url: string): string | null {
  if (!url) return null;

  // Support raw video IDs
  if (/^[a-zA-Z0-9_-]{11}$/.test(url)) return url;

  // Match various YouTube URL patterns
  const patterns = [
    /(?:youtube\.com\/watch\?v=|youtu\.be\/)([a-zA-Z0-9_-]{11})/,
    /youtube\.com\/embed\/([a-zA-Z0-9_-]{11})/,
  ];

  for (const pattern of patterns) {
    const match = url.match(pattern);
    if (match) return match[1];
  }

  return null;
}

// Canonicalize a YouTube video URL to `https://www.youtube.com/watch?v=ID`,
// stripping playlist/index/timestamp/si/pp parameters. Non-YouTube-video
// inputs (channels, playlists, file paths, raw text) pass through unchanged.
// Safe to call on any string — idempotent and a no-op for non-matches.
export function sanitizeUrl(input: string): string {
  const trimmed = (input ?? '').trim();
  if (!trimmed) return '';

  const videoId = extractVideoId(trimmed);
  if (videoId) {
    return `https://www.youtube.com/watch?v=${videoId}`;
  }

  return trimmed;
}

export function parseTimeframe(input: string): { value: number; unit: string } | null {
  const match = input.match(/^(\d+)([dwmy])$/);
  if (!match) return null;

  return {
    value: parseInt(match[1], 10),
    unit: match[2],
  };
}

export function formatTimeframe(value: number, unit: string): string {
  return `${value}${unit}`;
}
