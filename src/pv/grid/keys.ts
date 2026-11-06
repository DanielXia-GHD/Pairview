// Pairview: K in the library grid (docs/IMPLEMENTATION_PLAN.md M2-08): cycles the Keep-RAW lock
// Auto → Keep 🔒 → Drop for the selected Photos, like K in the viewer. Registered in the capture
// phase so it runs before upstream's K (AI panel) and stops it only when it acts.
import { toast } from 'react-toastify';
import { useLibraryStore } from '../../store/useLibraryStore';
import { useUIStore } from '../../store/useUIStore';
import { pvApi, type KeepRaw } from '../api';
import { pvPhotosFor } from '../delete/choose';
import { pvPhotoForPath, usePvPhotoStore } from '../store';
import { usePvViewer } from '../viewer/PvViewer';
import { refuseIfOffline } from '../offline';

const KEEP_CYCLE: KeepRaw[] = ['auto', 'keep', 'drop'];

const isTyping = () => {
  const el = document.activeElement;
  return el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement || (el as HTMLElement)?.isContentEditable;
};

let openEditor: (path: string) => void = () => {};

/** Upstream's file operations register how to open the editor. */
export function setPvOpenEditor(open: (path: string) => void) {
  openEditor = open;
}

/** E in the grid opens the editor on the active Photo's RAW (its grid path, A4), M3-01. */
function onEditKey(e: KeyboardEvent) {
  const { libraryActivePath, multiSelectedPaths } = useLibraryStore.getState();
  const path = libraryActivePath ?? multiSelectedPaths[0];
  if (!path || !pvPhotoForPath(path)) return;
  e.preventDefault();
  e.stopImmediatePropagation();
  openEditor(path);
}

export function onGridKey(e: KeyboardEvent) {
  if (e.key.toLowerCase() === 'e' && !e.metaKey && !e.ctrlKey && !e.altKey && !e.shiftKey) {
    if (useUIStore.getState().activeView === 'library' && !isTyping() && !usePvViewer.getState().path) onEditKey(e);
    return;
  }
  if (e.key.toLowerCase() !== 'k' || e.metaKey || e.ctrlKey || e.altKey) return;
  if (useUIStore.getState().activeView !== 'library' || isTyping() || usePvViewer.getState().path) return;
  const photos = pvPhotosFor(useLibraryStore.getState().multiSelectedPaths);
  if (!photos || photos.length === 0) return;
  // Upstream binds K to its AI panel; in the library a Pairview selection takes it.
  e.preventDefault();
  e.stopImmediatePropagation();
  if (refuseIfOffline(photos.map((p) => p.representativePath ?? p.displayPath))) return;
  const next = KEEP_CYCLE[(KEEP_CYCLE.indexOf(photos[0].keepRaw) + 1) % KEEP_CYCLE.length];
  void (async () => {
    try {
      const results = await pvApi.setMetadata(
        photos.map((p) => p.id),
        { keepRaw: next },
      );
      const failed = results.filter((r) => r.error);
      if (failed.length > 0) toast.error(failed[0].error);
      usePvPhotoStore.setState((s) => {
        const byPath = { ...s.byPath };
        for (const [path, photo] of Object.entries(byPath)) {
          const r = results.find((x) => x.photoId === photo.id && x.meta);
          if (r?.meta) byPath[path] = { ...photo, ...r.meta };
        }
        return { byPath };
      });
    } catch (err) {
      toast.error(String(err));
    }
  })();
}
