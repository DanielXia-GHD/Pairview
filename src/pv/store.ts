// Pairview: details of the Photos in the current library listing, keyed by the path the grid uses.
import { create } from 'zustand';
import type { PhotoItem } from './api';

interface PvPhotoState {
  byPath: Record<string, PhotoItem>;
  setPhotos(photos: PhotoItem[]): void;
  clear(): void;
}

/** The grid path of a Photo: its representative file (the RAW when there is one). */
export const gridPath = (photo: PhotoItem) => photo.representativePath ?? photo.displayPath;

/** Upstream virtual copies are `path?vc=<id>`; they share their Photo's details. */
export const basePath = (path: string) => path.split('?vc=')[0];

export const usePvPhotoStore = create<PvPhotoState>((set) => ({
  byPath: {},
  setPhotos: (photos) =>
    set({
      byPath: Object.fromEntries(
        photos.flatMap((p) => {
          const path = gridPath(p);
          return path ? [[path, p]] : [];
        }),
      ),
    }),
  clear: () => set({ byPath: {} }),
}));

export const pvPhotoForPath = (path: string): PhotoItem | undefined =>
  usePvPhotoStore.getState().byPath[basePath(path)];
