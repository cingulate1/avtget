import { create } from 'zustand';

export type ThemeMode = 'light' | 'navy' | 'dark';

interface ThemeState {
    theme: ThemeMode;
    setTheme: (theme: ThemeMode) => void;
}

export const useThemeStore = create<ThemeState>((set) => ({
    theme: 'navy',  // Default to semi-dark mode
    setTheme: (theme) => set({ theme }),
}));

// Theme color definitions
export const themes = {
    light: {
        bg: '#f3f4f6',           // gray-100
        cardBg: '#ffffff',       // white
        text: '#1f2937',         // gray-800
        textMuted: '#6b7280',    // gray-500
        border: '#d1d5db',       // gray-300
        buttonPrimary: '#2563eb', // blue-600
        buttonPrimaryHover: '#1d4ed8', // blue-700
        buttonDisabled: '#1e40af', // blue-800 (darker blue)
        buttonDanger: '#dc2626', // red-600
        buttonSecondary: '#4b5563', // gray-600
        inputBg: '#ffffff',
        inputBorder: '#d1d5db',
        headerBg: '#e5e7eb',     // gray-200
    },
    navy: {
        bg: '#0a1120',           // darker navy
        cardBg: '#151d2e',       // darker card bg
        text: '#f1f5f9',         // slate-100
        textMuted: '#94a3b8',    // slate-400
        border: '#283548',       // darker border
        buttonPrimary: '#F60064', // hot pink
        buttonPrimaryHover: '#c50050', // darker hot pink
        buttonDisabled: '#7d0032', // much darker hot pink
        buttonDanger: '#F60064', // hot pink
        buttonSecondary: '#4a5568', // darker slate
        inputBg: '#151d2e',
        inputBorder: '#384660',
        headerBg: '#283548',     // darker header
    },
    dark: {
        bg: '#000000',           // black
        cardBg: '#0a0a0a',       // near-black
        text: '#f0fdf4',         // green-50
        textMuted: '#86efac',    // green-300
        border: '#166534',       // green-800
        buttonPrimary: '#22c55e', // green-500
        buttonPrimaryHover: '#16a34a', // green-600
        buttonDisabled: '#166534', // green-800 (darker green)
        buttonDanger: '#22c55e', // green-500
        buttonSecondary: '#166534', // green-800
        inputBg: '#0a0a0a',
        inputBorder: '#166534',
        headerBg: '#052e16',     // green-950
    },
};
