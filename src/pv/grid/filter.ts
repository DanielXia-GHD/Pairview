// Pairview: filters added to the library's filter menu (docs/IMPLEMENTATION_PLAN.md M1-12).
import { pvPhotoForPath } from '../store';
import type { PhotoItem } from '../api';

export type PvPhotoFilter = 'all' | 'pairs' | 'displayOnly' | 'rawOnly' | 'orphans' | 'mismatch' | 'edited';
export type PvKeepRawFilter = 'all' | 'auto' | 'keep' | 'drop';

export interface PvFilter {
  photo?: PvPhotoFilter;
  keepRaw?: PvKeepRawFilter;
}

export const PV_PHOTO_FILTERS: PvPhotoFilter[] = ['all', 'pairs', 'displayOnly', 'rawOnly', 'orphans', 'mismatch'];
export const PV_KEEP_RAW_FILTERS: PvKeepRawFilter[] = ['all', 'auto', 'keep', 'drop'];

export const pvFilterActive = (f?: PvFilter) => (f?.photo ?? 'all') !== 'all' || (f?.keepRaw ?? 'all') !== 'all';

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

/** For upstream's filter: images that aren't Pairview Photos only pass when no filter is set. */
export function pvMatchesFilter(path: string, f?: PvFilter): boolean {
  if (!pvFilterActive(f)) return true;
  const photo = pvPhotoForPath(path);
  if (!photo) return false;
  if (!matchesPhoto(photo, f?.photo ?? 'all')) return false;
  const keep = f?.keepRaw ?? 'all';
  return keep === 'all' || photo.keepRaw === keep;
}
