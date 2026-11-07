// Pairview: AI triage suggestions for the open shoot (docs/M4-triage.md). Kept apart from the Photo
// store and from the owner's ratings and flags: these are only ever suggestions.
import { create } from 'zustand';
import type { PhotoItem, ShootTriage, TriageSuggestion } from '../api';

interface PvTriageState {
  byPhoto: Record<number, TriageSuggestion>;
  setResults(results: ShootTriage | null): void;
}

export const usePvTriageStore = create<PvTriageState>((set) => ({
  byPhoto: {},
  setResults: (results) =>
    set({ byPhoto: Object.fromEntries((results?.suggestions ?? []).map((s) => [s.photoId, s])) }),
}));

/**
 * The backend leaves out anything the owner kept, but a rating can arrive after the results did.
 * Human input wins at once, so this is checked again at display time.
 */
export const stillSuggestedReject = (photo: PhotoItem, s: TriageSuggestion | undefined) =>
  !!s?.reject && photo.rating === 0 && photo.flag === 'none' && photo.keepRaw !== 'keep';
