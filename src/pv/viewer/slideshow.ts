// Pairview: slideshow options and order (PRD §6.2, docs/IMPLEMENTATION_PLAN.md M1-15).
import { useCallback, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { computeSortedLibrary } from '../../hooks/useSortedLibrary';
import { useLibraryStore } from '../../store/useLibraryStore';
import { useSettingsStore } from '../../store/useSettingsStore';
import type { ImageFile } from '../../components/ui/AppProperties';
import type { PhotoItem } from '../api';
import { isPvDatesPath } from '../dates';
import { pvPhotoForPath, usePvPhotoStore } from '../store';

export interface SlideshowOptions {
  seconds: number;
  fade: boolean;
  shuffle: boolean;
  loop: boolean;
  /** Include the Photos of sub-folders (with the grid's filter and sort applied). */
  subfolders: boolean;
}

export const SLIDESHOW_SECONDS = [2, 3, 5, 8, 15];

const DEFAULTS: SlideshowOptions = { seconds: 5, fade: true, shuffle: false, loop: true, subfolders: true };
const STORAGE_KEY = 'pv.slideshow';

function load(): SlideshowOptions {
  try {
    return { ...DEFAULTS, ...JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '{}') };
  } catch {
    return DEFAULTS;
  }
}

/** Options remembered on this computer between slideshows. */
export function useSlideshowOptions(): [SlideshowOptions, (patch: Partial<SlideshowOptions>) => void] {
  const [options, setOptions] = useState(load);
  const update = useCallback((patch: Partial<SlideshowOptions>) => {
    setOptions((prev) => {
      const next = { ...prev, ...patch };
      try {
        localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
      } catch {
        // Private mode / blocked storage: options just aren't remembered.
      }
      return next;
    });
  }, []);
  return [options, update];
}

/** `paths` in random order, with `first` (if present) kept at the start. */
export function shuffled(paths: string[], first?: string): string[] {
  const rest = paths.filter((p) => p !== first);
  for (let i = rest.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [rest[i], rest[j]] = [rest[j], rest[i]];
  }
  return first && paths.includes(first) ? [first, ...rest] : rest;
}

/**
 * The current folder's Photos including sub-folders, filtered and sorted exactly like the grid,
 * or null when the library isn't showing a Pairview folder (dates, albums, other folders).
 */
export async function recursivePaths(): Promise<string[] | null> {
  const library = useLibraryStore.getState();
  const folder = library.currentFolderPath;
  if (!folder || folder.startsWith('Album: ') || isPvDatesPath(folder)) return null;
  const listing = await invoke<{ images: ImageFile[]; photos: PhotoItem[] } | null>('pv_list_images_for_path', {
    path: folder,
    recursive: true,
  });
  if (!listing) return null;
  usePvPhotoStore.getState().mergePhotos(listing.photos);
  const ratings = Object.fromEntries(listing.images.map((f) => [f.path, f.rating]));
  const sorted = computeSortedLibrary(
    { ...library, imageList: listing.images, imageRatings: { ...library.imageRatings, ...ratings } },
    useSettingsStore.getState(),
  );
  return sorted.map((f) => f.path).filter((p) => pvPhotoForPath(p));
}
