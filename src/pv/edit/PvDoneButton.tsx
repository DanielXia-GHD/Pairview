// Pairview: Done in the editor (docs/IMPLEMENTATION_PLAN.md M3-02, PRD §6.4). Renders the RAW +
// recipe at full resolution and replaces the display copy in its own format; the old one goes to
// Recently Deleted (a Photo without a RAW keeps its original as a hidden base instead). Revert puts
// the original back, M3-03.
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Check, Loader2, RotateCcw } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'react-toastify';
import { useEditorStore } from '../../store/useEditorStore';
import { debouncedSave } from '../../hooks/useEditorActions';
import { useUIStore } from '../../store/useUIStore';
import { globalImageCache } from '../../utils/ImageLRUCache';
import { formatBytes } from '../../utils/formatBytes';
import { refuseIfOffline } from '../offline';
import { pvPhotoForPath, bumpDisplayVersion, usePvPhotoStore } from '../store';

interface ReplacedResult {
  opId: string;
  locationId: number;
  replacedBytes: number;
  newBytes: number;
  keptAsBase: boolean;
}

export default function PvDoneButton({ onDone }: { onDone(): void }) {
  const { t } = useTranslation();
  const path = useEditorStore((s) => s.selectedImage?.path);
  // Re-render when the Photo's details arrive.
  usePvPhotoStore((s) => s.byPath);
  const [busy, setBusy] = useState(false);
  const [canRevert, setCanRevert] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setCanRevert(false);
    if (!path || path.includes('?vc=')) return;
    invoke<boolean>('pv_edit_can_revert', { path })
      .then((yes) => !cancelled && setCanRevert(yes))
      .catch(() => !cancelled && setCanRevert(false));
    return () => {
      cancelled = true;
    };
  }, [path]);

  const photo = path && !path.includes('?vc=') ? pvPhotoForPath(path) : undefined;
  if (!path || !photo?.displayExt) return null;
  const display = photo.displayExt.toUpperCase();

  const done = async () => {
    if (busy || refuseIfOffline([path])) return;
    setBusy(true);
    try {
      debouncedSave.flush();
      const r = await invoke<ReplacedResult>('pv_edit_done', {
        path,
        adjustments: useEditorStore.getState().adjustments,
      });
      bumpDisplayVersion(photo.id);
      if (photo.rawExt) {
        usePvPhotoStore.setState((s) => ({ byPath: { ...s.byPath, [path]: { ...photo, state: 'edited' } } }));
      }
      setCanRevert(true);
      toast.success(
        t(r.keptAsBase ? 'pv.edit.doneBase' : 'pv.edit.done', { display, size: formatBytes(r.newBytes, t, 1) }),
      );
      onDone();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  const revert = () => {
    useUIStore.getState().setUI({
      confirmModalState: {
        isOpen: true,
        title: t('pv.edit.revertTitle'),
        message: t('pv.edit.revertMessage', { display }),
        confirmText: t('pv.edit.revertConfirm'),
        confirmVariant: 'destructive',
        onConfirm: async () => {
          setBusy(true);
          try {
            await invoke('pv_edit_revert', { path });
            bumpDisplayVersion(photo.id);
            // The editor must not reopen on the recipe that just went away.
            globalImageCache.deleteByPrefix(path);
            usePvPhotoStore.setState((s) => ({
              byPath: {
                ...s.byPath,
                [path]: { ...photo, state: photo.rawExt ? 'raw_display' : 'display_only' },
              },
            }));
            setCanRevert(false);
            toast.success(t('pv.edit.reverted', { display }));
            onDone();
          } catch (e) {
            toast.error(String(e));
          } finally {
            setBusy(false);
          }
        },
      },
    });
  };

  return (
    <>
      {canRevert && (
        <button
          type="button"
          className="flex items-center gap-1.5 rounded-full bg-surface px-3 py-1.5 text-sm text-text-primary transition-colors hover:bg-card-active disabled:opacity-60"
          onClick={revert}
          disabled={busy}
          data-tooltip={t('pv.edit.revertHint', { display })}
        >
          <RotateCcw size={16} />
          {t('pv.edit.revert')}
        </button>
      )}
      <button
        type="button"
        className="flex items-center gap-1.5 rounded-full bg-accent px-3 py-1.5 text-sm font-semibold text-button-text transition-colors hover:bg-accent/90 disabled:opacity-60"
        onClick={() => void done()}
        disabled={busy}
        data-tooltip={t('pv.edit.hint', { display })}
      >
        {busy ? <Loader2 size={16} className="animate-spin" /> : <Check size={16} />}
        {busy ? t('pv.edit.rendering') : t('pv.edit.button')}
      </button>
    </>
  );
}
