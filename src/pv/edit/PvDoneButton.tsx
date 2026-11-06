// Pairview: Done in the editor (docs/IMPLEMENTATION_PLAN.md M3-02, PRD §6.4). Renders the RAW +
// recipe at full resolution and replaces the display copy in its own format; the old one goes to
// Recently Deleted. Shown only for Photos with both a RAW and a display copy (M3-03 adds the rest).
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Check, Loader2 } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'react-toastify';
import { useEditorStore } from '../../store/useEditorStore';
import { debouncedSave } from '../../hooks/useEditorActions';
import { formatBytes } from '../../utils/formatBytes';
import { refuseIfOffline } from '../offline';
import { pvPhotoForPath, bumpDisplayVersion, usePvPhotoStore } from '../store';

interface ReplacedResult {
  opId: string;
  locationId: number;
  replacedBytes: number;
  newBytes: number;
}

export default function PvDoneButton({ onDone }: { onDone(): void }) {
  const { t } = useTranslation();
  const path = useEditorStore((s) => s.selectedImage?.path);
  // Re-render when the Photo's details arrive.
  usePvPhotoStore((s) => s.byPath);
  const [busy, setBusy] = useState(false);

  const photo = path && !path.includes('?vc=') ? pvPhotoForPath(path) : undefined;
  if (!path || !photo?.rawExt || !photo.displayExt) return null;
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
      usePvPhotoStore.setState((s) => ({ byPath: { ...s.byPath, [path]: { ...photo, state: 'edited' } } }));
      toast.success(t('pv.edit.done', { display, size: formatBytes(r.newBytes, t, 1) }));
      onDone();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
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
  );
}
