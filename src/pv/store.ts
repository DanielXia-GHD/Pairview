// Pairview: details of the Photos in the current library listing, keyed by the path the grid uses.
import { create } from 'zustand';
import type { PhotoItem } from './api';

interface PvPhotoState {
  byPath: Record<string, PhotoItem>;
  setPhotos(photos: PhotoItem[]): void;
  /** Adds Photos (e.g. a slideshow's sub-folders) without dropping the grid's. */
  mergePhotos(photos: PhotoItem[]): void;
  clear(): void;
}

/** The grid path of a Photo: its representative file (the RAW when there is one). */
export const gridPath = (photo: PhotoItem) => photo.representativePath ?? photo.displayPath;

/** Upstream virtual copies are `path?vc=<id>`; they share their Photo's details. */
export const basePath = (path: string) => path.split('?vc=')[0];

const index = (photos: PhotoItem[]) =>
  Object.fromEntries(
    photos.flatMap((p) => {
      const path = gridPath(p);
      return path ? [[path, p]] : [];
    }),
  );

export const usePvPhotoStore = create<PvPhotoState>((set) => ({
  byPath: {},
  setPhotos: (photos) => set({ byPath: index(photos) }),
  mergePhotos: (photos) => set((s) => ({ byPath: { ...s.byPath, ...index(photos) } })),
  clear: () => set({ byPath: {} }),
}));

// Done replaces a display copy under the same URL; the version keeps the webview from showing the
// old image for the rest of the session.
const displayVersions = new Map<number, number>();
export const bumpDisplayVersion = (photoId: number) =>
  displayVersions.set(photoId, (displayVersions.get(photoId) ?? 0) + 1);
export const displayVersion = (photoId: number) => displayVersions.get(photoId) ?? 0;

export const pvPhotoForPath = (path: string): PhotoItem | undefined =>
  usePvPhotoStore.getState().byPath[basePath(path)];
