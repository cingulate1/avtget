import type { ArtifactStatus } from '@/shared/types';

export function getStatusEmoji(status: ArtifactStatus): string {
  switch (status) {
    case 'not_requested':
      return '—';
    case 'skipped':
      return '➖';  // Skipped (e.g., V/A for local audio file)
    case 'queued':
      return '⏳';
    case 'running':
      return '🔄';  // Processing spinner, not play button
    case 'completed':
      return '✅';
    case 'warning':
      return '⚠️';  // Model overloaded - raw transcript saved
    case 'failed':
      return '❌';
    case 'cancelled':
      return '❌';
    default:
      return '?';
  }
}

// No color-coded text - return empty string for all statuses
export function getStatusColor(_status: ArtifactStatus): string {
  return '';  // All text is black, no color coding
}

export function getRowBackgroundColor(status: ArtifactStatus): string {
  switch (status) {
    case 'running':
      return 'bg-blue-50/50';  // Very light blue for processing
    case 'completed':
      return 'bg-green-50/50';  // Very light green for success
    case 'warning':
      return 'bg-yellow-50/50';  // Very light yellow for warning (raw transcript saved)
    case 'failed':
    case 'cancelled':
      return 'bg-red-50/50';  // Very light red for failed/cancelled
    default:
      return '';  // No fill for loading/queued
  }
}
