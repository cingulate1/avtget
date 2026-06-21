import { useEffect, useRef, useState } from 'react';
import { isChannelUrl, isPlaylistUrl, isPodcastUrl } from '../utils/youtube';
import { useThemeStore, themes } from '../store/themeStore';
import { useLogStore } from '../store/logStore';
import { getDesktopAPI } from '../desktopApi';

// File extensions — must stay in sync with routing.rs AUDIO_EXTENSIONS / VIDEO_EXTENSIONS
const AUDIO_EXTENSIONS = ['mp3', 'wav', 'flac', 'm4a', 'ogg', 'opus', 'wma', 'aac', 'aiff', 'alac'];
const VIDEO_EXTENSIONS = ['mp4', 'mkv', 'avi', 'mov', 'm4v', 'webm', 'wmv', 'flv', 'ts', 'm2ts', 'mpeg', 'mpg', '3gp', 'vob'];

interface InputFieldsProps {
  inputs: string[];
  setInputs: (inputs: string[]) => void;
  setShowTimeframe: (show: boolean) => void;
  setShowEpisodeLimit: (show: boolean) => void;
}

export function InputFields({ inputs, setInputs, setShowTimeframe, setShowEpisodeLimit }: InputFieldsProps) {
  const desktopAPI = getDesktopAPI();
  const currentTheme = useThemeStore((s) => s.theme);
  const themeColors = themes[currentTheme];
  const [isDragOver, setIsDragOver] = useState(false);

  // Keep a ref so async callbacks (onFileDrop effect) can read the latest
  // inputs without a stale closure — setInputs doesn't accept a functional
  // updater, so we need a current snapshot.
  const inputsRef = useRef(inputs);
  useEffect(() => { inputsRef.current = inputs; }, [inputs]);

  // Auto-manage N+1 pattern
  useEffect(() => {
    const hasEmpty = inputs.some((input) => input.trim() === '');
    const lastIsEmpty = inputs[inputs.length - 1]?.trim() === '';

    if (!hasEmpty || !lastIsEmpty) {
      setInputs([...inputs, '']);
    }
  }, [inputs, setInputs]);

  // Detect channel URLs and podcast URLs
  useEffect(() => {
    const hasChannelUrl = inputs.some((input) => isChannelUrl(input));
    const hasPodcastUrl = inputs.some((input) => isPodcastUrl(input));
    setShowTimeframe(hasChannelUrl && !hasPodcastUrl);
    setShowEpisodeLimit(hasPodcastUrl);
  }, [inputs, setShowTimeframe, setShowEpisodeLimit]);

  // Playlist expansion: whenever a playlist URL lands in the input array (via
  // typing, paste, file drop, .txt list, or Firefox extension), shell out to
  // `yt-dlp --flat-playlist --print url` and splice the resulting video URLs
  // into the field at the same index. If the field is edited before the
  // expansion returns, the result is discarded silently.
  const expandingRef = useRef<Set<string>>(new Set());
  useEffect(() => {
    inputs.forEach((input, index) => {
      if (!isPlaylistUrl(input)) return;
      if (expandingRef.current.has(input)) return;
      expandingRef.current.add(input);
      desktopAPI
        .expandPlaylist(input)
        .then((expandedUrls) => {
          const current = inputsRef.current;
          const stillHere = current.indexOf(input);
          if (stillHere === -1) return;
          const next = [...current];
          next.splice(stillHere, 1, ...expandedUrls);
          if (next[next.length - 1]?.trim() !== '') next.push('');
          setInputs(next);
        })
        .catch((err) => {
          useLogStore.getState().addLog(`Playlist expansion failed: ${err}`, true);
        })
        .finally(() => {
          expandingRef.current.delete(input);
        });
    });
    // desktopAPI is stable; inputsRef/setInputs read current values at
    // resolution time so staleness is not a concern.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [inputs]);

  // Insert a path at a specific index, preserving the N+1 invariant.
  // Always reads from inputsRef so it's safe to call from async contexts.
  const insertPath = (filePath: string, index: number) => {
    const current = inputsRef.current;
    const newInputs = [...current];
    if (index < newInputs.length) {
      newInputs[index] = filePath;
    } else {
      newInputs.push(filePath);
    }
    setInputs(newInputs.filter((inp, i) => {
      if (i === newInputs.length - 1) return true;
      return inp.trim() !== '';
    }));
  };

  // Handle a single resolved file path — shared by browse button and drop handler.
  const handleFileSelected = async (filePath: string, targetIndex?: number) => {
    const ext = filePath.split('.').pop()?.toLowerCase() || '';

    // Insertion index: use provided value, else first empty slot.
    const resolveIndex = () => {
      if (targetIndex !== undefined) return targetIndex;
      const current = inputsRef.current;
      const idx = current.findIndex((inp) => inp.trim() === '');
      return idx >= 0 ? idx : current.length;
    };

    if (ext === 'txt') {
      try {
        const content = await desktopAPI.readTextFile(filePath);
        if (content) {
          if (content.startsWith('https')) {
            // URL list — per line: keep only lines that start with "https",
            // take the first whitespace/comma-separated token.
            const urls = content
              .split(/\r?\n/)
              .map((line) => line.trim())
              .filter((line) => line.startsWith('https'))
              .map((line) => line.split(/[\s,]+/)[0])
              .filter(Boolean);
            if (urls.length > 0) {
              const idx = resolveIndex();
              const newInputs = [...inputsRef.current];
              newInputs.splice(idx, 1, ...urls);
              if (newInputs[newInputs.length - 1]?.trim() !== '') {
                newInputs.push('');
              }
              setInputs(newInputs);
            }
          } else {
            // Transcript file
            const idx = resolveIndex();
            const settings = await desktopAPI.getConfig();
            const autoClean = settings?.auto_clean_transcript || 'off';
            const autoCleanEnabled =
              autoClean &&
              !['off', 'false', 'no', '0', 'none', 'disabled'].includes(autoClean.toLowerCase());

            if (autoCleanEnabled) {
              insertPath(filePath, idx);
            } else {
              const fileName = filePath.split(/[/\\]/).pop() || 'file';
              const stem = fileName.replace(/\.txt$/i, '');
              const result = await desktopAPI.showMessageBox({
                type: 'question',
                buttons: ['Yes', 'No'],
                defaultId: 0,
                title: 'Clean Transcript?',
                message: `Clean the attached transcript - ${stem}.txt?`,
                detail:
                  'If Yes, the transcript will be cleaned using Ollama. If No, the file will be skipped.',
              });
              const { useJobStore } = await import('../store/jobStore');
              insertPath(filePath, idx);
              if (result === 0) {
                useJobStore.getState().addManualCleanInput(filePath);
              } else {
                useJobStore.getState().addSkipInput(filePath);
              }
            }
          }
        }
      } catch (error) {
        console.error('Failed to read text file:', error);
      }
    } else {
      insertPath(filePath, resolveIndex());
    }
  };

  const handleChange = (index: number, value: string) => {
    const newInputs = [...inputs];
    newInputs[index] = value;
    setInputs(newInputs.filter((inp, i) => {
      if (i === newInputs.length - 1) return true;
      return inp.trim() !== '';
    }));
  };

  const handleBrowse = async (index: number) => {
    const filePath = await desktopAPI.showOpenDialog({
      properties: ['openFile'],
      filters: [
        { name: 'Media Files', extensions: [...VIDEO_EXTENSIONS, ...AUDIO_EXTENSIONS, 'txt'] },
        { name: 'Text Files', extensions: ['txt'] },
        { name: 'All Files', extensions: ['*'] },
      ],
    });

    if (filePath) {
      await handleFileSelected(filePath, index);
    }
  };

  // Subscribe to Tauri's OS-level file drop events.
  // Standard HTML5 drag events don't fire for file drops in Tauri v2 because
  // the native handler intercepts them first. onFileDrop wraps onDragDropEvent.
  useEffect(() => {
    const unlisten = desktopAPI.onFileDrop(
      async (paths) => {
        if (paths.length === 0) return;

        if (paths.length === 1) {
          // Single file: full handling including txt analysis
          await handleFileSelected(paths[0]);
          return;
        }

        // Multiple files: batch insert all paths, skipping txt analysis
        const filled = inputsRef.current.filter((inp) => inp.trim() !== '');
        setInputs([...filled, ...paths, '']);
      },
      (dragging) => setIsDragOver(dragging),
    );

    return unlisten;
    // desktopAPI is stable; handleFileSelected intentionally excluded —
    // it reads inputsRef.current at call time so staleness is not a concern.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [desktopAPI]);

  return (
    <div
      className="flex flex-col gap-1"
      style={{
        borderRadius: '8px',
        outline: isDragOver
          ? `2px dashed ${themeColors.buttonPrimary}`
          : '2px solid transparent',
        outlineOffset: '4px',
        transition: 'outline-color 120ms',
      }}
    >
      {inputs.map((input, index) => (
        <div key={index} className="flex gap-1">
          <input
            type="text"
            value={input}
            onChange={(e) => handleChange(index, e.target.value)}
            placeholder="Enter video URL, podcast feed, video ID, or local file path (video/audio)"
            className="flex-1 px-3 py-2 rounded-md focus:outline-none focus:ring-2 transition-all duration-[400ms]"
            style={{
              backgroundColor: themeColors.inputBg,
              borderWidth: '1px',
              borderStyle: 'solid',
              borderColor: themeColors.inputBorder,
              color: themeColors.text,
            }}
          />
          <button
            onClick={() => handleBrowse(index)}
            className="px-2 py-2 rounded-md transition-transform duration-[80ms] hover:brightness-110 active:scale-95 active:brightness-90"
            style={{
              backgroundColor: themeColors.buttonSecondary || '#6b7280',
              color: 'white',
              aspectRatio: '1',
              minWidth: '38px',
            }}
            title="Browse for file"
          >
            ...
          </button>
        </div>
      ))}
    </div>
  );
}
