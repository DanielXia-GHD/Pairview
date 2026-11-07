// Pairview: filters added to the library's filter menu (docs/IMPLEMENTATION_PLAN.md M1-12).
import { pvPhotoForPath } from '../store';
import type { PhotoItem } from '../api';
import { stillSuggestedReject, usePvTriageStore } from './triage';

export type PvPhotoFilter = 'all' | 'pairs' | 'displayOnly' | 'rawOnly' | 'orphans' | 'mismatch' | 'edited';
export type PvKeepRawFilter = 'all' | 'auto' | 'keep' | 'drop';
export type PvAiFilter = 'all' | 'rejects' | 'best' | 'bursts';

export interface PvFilter {
  photo?: PvPhotoFilter;
  keepRaw?: PvKeepRawFilter;
  ai?: PvAiFilter;
}

export const PV_PHOTO_FILTERS: PvPhotoFilter[] = ['all', 'pairs', 'displayOnly', 'rawOnly', 'orphans', 'mismatch'];
export const PV_KEEP_RAW_FILTERS: PvKeepRawFilter[] = ['all', 'auto', 'keep', 'drop'];
export const PV_AI_FILTERS: PvAiFilter[] = ['all', 'rejects', 'best', 'bursts'];

export const pvFilterActive = (f?: PvFilter) =>
  (f?.photo ?? 'all') !== 'all' || (f?.keepRaw ?? 'all') !== 'all' || (f?.ai ?? 'all') !== 'all';

const matchesPhoto = (p: PhotoItem, filter: PvPhotoFilter) => {
  switch (filter) {
    case 'all':
      return true;
    case 'pairs':
      return p.state === 'raw_display' || p.state === 'edited';
    case 'displayOnly':
      return p.state === 'display_only';
    case 'rawOnly':
      return p.state === 'raw_only';
    case 'orphans':
      return p.state === 'orphan_raw';
    case 'mismatch':
      return p.guardMismatch;
    case 'edited':
      return p.state === 'edited';
  }
};

const matchesAi = (p: PhotoItem, filter: PvAiFilter) => {
  if (filter === 'all') return true;
  const s = usePvTriageStore.getState().byPhoto[p.id];
  switch (filter) {
    case 'rejects':
      return stillSuggestedReject(p, s);
    case 'best':
      return !!s?.bestOfBurst;
    case 'bursts':
      return (s?.burstSize ?? 0) > 1;
  }
};

/** For upstream's filter: images that aren't Pairview Photos only pass when no filter is set. */
export function pvMatchesFilter(path: string, f?: PvFilter): boolean {
  if (!pvFilterActive(f)) return true;
  const photo = pvPhotoForPath(path);
  if (!photo) return false;
  if (!matchesPhoto(photo, f?.photo ?? 'all')) return false;
  if (!matchesAi(photo, f?.ai ?? 'all')) return false;
  const keep = f?.keepRaw ?? 'all';
  return keep === 'all' || photo.keepRaw === keep;
}
