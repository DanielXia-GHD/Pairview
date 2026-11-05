// Pairview: which delete a Photo gets (docs/IMPLEMENTATION_PLAN.md M2-03, PRD §6.6).
//
// Upstream's executeDelete asks `chooseDelete` first. For Pairview Photos it answers with the
// owner's choice from the delete dialog, or the remembered default; for anything else it
// answers `null` and upstream deletes as before.
import { create } from 'zustand';
import { pvPhotoForPath } from '../store';
import type { DeleteScope, PhotoItem } from '../api';

/** What Delete does on a RAW+display Photo: ask, or one of the three choices. */
export type DeleteDefault = 'ask' | DeleteScope;

const STORAGE_KEY = 'pv.delete.default';
const DEFAULTS: DeleteDefault[] = ['ask', 'photo', 'keepDisplay', 'keepRaw'];

export function loadDeleteDefault(): DeleteDefault {
  try {
    const value = localStorage.getItem(STORAGE_KEY) as DeleteDefault | null;
    return value && DEFAULTS.includes(value) ? value : 'ask';
  } catch {
    return 'ask';
  }
}

export function saveDeleteDefault(value: DeleteDefault) {
  try {
    localStorage.setItem(STORAGE_KEY, value);
  } catch {
    // Private storage: the dialog keeps asking, which is the safe side.
  }
  useDeleteDialog.setState({ remembered: value });
}

interface DeleteRequest {
  photos: PhotoItem[];
  resolve(choice: DeleteScope | 'cancel'): void;
}

interface DeleteDialogState {
  request: DeleteRequest | null;
  remembered: DeleteDefault;
}

export const useDeleteDialog = create<DeleteDialogState>(() => ({
  request: null,
  remembered: loadDeleteDefault(),
}));

/** The Pairview Photos for grid paths, or null when any path is something else (or a virtual copy). */
export function pvPhotosFor(paths: string[]): PhotoItem[] | null {
  if (paths.length === 0 || paths.some((p) => p.includes('?vc='))) return null;
  const photos = paths.map(pvPhotoForPath);
  return photos.every(Boolean) ? (photos as PhotoItem[]) : null;
}

/**
 * The delete for `paths`: a scope for Pairview Photos, 'cancel' when the owner closed the
 * dialog, or null to let upstream delete them as before.
 */
export async function chooseDelete(paths: string[], withAssociated: boolean): Promise<DeleteScope | 'cancel' | null> {
  const photos = pvPhotosFor(paths);
  if (!photos) return null;
  // "Delete with associated files" is upstream's own whole-Photo delete, already confirmed.
  if (withAssociated) return 'photo';
  const remembered = loadDeleteDefault();
  if (remembered !== 'ask') return remembered;
  return new Promise((resolve) => {
    // A dialog already open is cancelled by a new request.
    useDeleteDialog.getState().request?.resolve('cancel');
    useDeleteDialog.setState({
      request: {
        photos,
        resolve: (choice) => {
          useDeleteDialog.setState({ request: null });
          resolve(choice);
        },
      },
    });
  });
}
