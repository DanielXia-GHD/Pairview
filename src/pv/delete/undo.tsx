// Pairview: Undo for deletes (docs/IMPLEMENTATION_PLAN.md M2-06, PRD §6.6).
// Cmd/Ctrl+Z in the library, or Undo on the "Moved to Recently Deleted" toast, restores the most
// recent delete (every file of that action). Pressing it again undoes the one before, until purge.
import { toast } from 'react-toastify';
import i18n from 'i18next';
import { usePvViewer } from '../viewer/PvViewer';
import { useUIStore } from '../../store/useUIStore';
import { useDeleteDialog } from './choose';
import { pvApi } from '../api';

let refreshLibrary: () => Promise<void> | void = () => {};

/** Upstream's file operations register how to reload the grid after an undo. */
export function setPvRefresh(refresh: () => Promise<void> | void) {
  refreshLibrary = refresh;
}

export async function pvUndo(): Promise<boolean> {
  try {
    const undone = await pvApi.undo();
    if (!undone) {
      toast.info(i18n.t('pv.undo.nothing'));
      return false;
    }
    toast.success(i18n.t('pv.undo.done', { count: undone.files }));
    if (undone.conflicts.length > 0) toast.warn(i18n.t('pv.deleted.conflicts', { count: undone.conflicts.length }));
    await refreshLibrary();
    return true;
  } catch (e) {
    toast.error(String(e));
    return false;
  }
}

/** "Moved N photos to Recently Deleted" with an Undo button. */
export function toastDeleted(message: string) {
  toast.success(
    ({ closeToast }) => (
      <div className="flex items-center gap-3">
        <span className="flex-1">{message}</span>
        <button
          type="button"
          className="rounded px-2 py-1 font-semibold underline-offset-2 hover:underline"
          onClick={() => {
            closeToast?.();
            void pvUndo();
          }}
        >
          {i18n.t('pv.undo.button')}
        </button>
      </div>
    ),
    { autoClose: 8000 },
  );
}

const isTyping = () => {
  const el = document.activeElement;
  return el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement || (el as HTMLElement)?.isContentEditable;
};

/** Cmd/Ctrl+Z in the library (not the editor, which has its own undo). */
export function onUndoKey(e: KeyboardEvent) {
  if (!(e.metaKey || e.ctrlKey) || e.shiftKey || e.altKey || e.key.toLowerCase() !== 'z') return;
  if (useUIStore.getState().activeView !== 'library' || isTyping()) return;
  if (usePvViewer.getState().path || useDeleteDialog.getState().request) return;
  if (useUIStore.getState().confirmModalState?.isOpen) return;
  e.preventDefault();
  void pvUndo();
}
