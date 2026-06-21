from enum import Enum


class VideoSource(Enum):
    YOUTUBE = "youtube"
    TWITCH = "twitch"
    LOCAL = "local"
    LOCAL_AUDIO = "local_audio"  # Local audio file for transcript-only
    LOCAL_VIDEO = "local_video"  # Local video file for audio extraction and transcription
    TRANSCRIPT = "transcript"  # Direct transcript file for cleaning only
    OVERCAST = "overcast"  # Overcast episode URL
    PODCAST = "podcast"  # Podcast RSS feed
    DIRECT_AUDIO = "direct_audio"  # Direct audio URL (e.g., https://example.com/file.mp3)
    UNKNOWN = "unknown"
