// Pairview: the delete dialog (docs/IMPLEMENTATION_PLAN.md M2-03, PRD §6.6).
// Delete photo · Keep HIF only · Keep RAW only, with sizes, and "don't ask again".
// Everything goes to Recently Deleted for 30 days.
import { useCallback, useEffect, useMemo, useState } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import clsx from 'clsx';
import Button from '../../components/ui/Button';
import Text from '../../components/ui/Text';
import { TextVariants } from '../../types/typography';
import { formatBytes } from '../../utils/formatBytes';
import { pvApi, type DeleteOption, type DeletePreview, type DeleteScope } from '../api';
import { saveDeleteDefault, useDeleteDialog } from './choose';
import { onUndoKey } from './undo';
import { onGridKey } from '../grid/keys';

const SCOPES: DeleteScope[] = ['photo', 'keepDisplay', 'keepRaw'];

export default function PvDeleteDialog() {
  const { t } = useTranslation();

  // Cmd/Ctrl+Z in the library undoes the last delete (M2-06).
  useEffect(() => {
    // K in the grid cycles the Keep-RAW lock (M2-08).
    window.addEventListener('keydown', onUndoKey);
    window.addEventListener('keydown', onGridKey, true);
    return () => {
      window.removeEventListener('keydown', onUndoKey);
      window.removeEventListener('keydown', onGridKey, true);
    };
  }, []);
  const request = useDeleteDialog((s) => s.request);
  const [preview, setPreview] = useState<DeletePreview | null>(null);
  const [failed, setFailed] = useState(false);
  const [scope, setScope] = useState<DeleteScope>('photo');
  const [dontAsk, setDontAsk] = useState(false);

  useEffect(() => {
    if (!request) return;
    let cancelled = false;
    setPreview(null);
    setFailed(false);
    setScope('photo');
    setDontAsk(false);
    pvApi
      .deletePreview(request.photos.map((p) => p.id))
      .then((p) => !cancelled && setPreview(p))
      .catch(() => !cancelled && setFailed(true));
    return () => {
      cancelled = true;
    };
  }, [request]);

  // "HIF", "JPG" or "HIF/JPG", from the pairs in the selection.
  const displayName = useMemo(() => {
    const exts = new Set(
      (request?.photos ?? [])
        .filter((p) => p.rawExt && p.displayExt)
        .map((p) => (p.displayExt as string).toUpperCase()),
    );
    return exts.size > 0 ? [...exts].sort().join('/') : t('pv.delete.displayCopy');
  }, [request, t]);

  const choices = preview && preview.pairs > 0 ? SCOPES : (['photo'] as DeleteScope[]);
  const optionOf = (s: DeleteScope): DeleteOption | undefined => preview?.[s];
  const canConfirm = !!preview && (optionOf(scope)?.files ?? 0) > 0;

  const close = useCallback(() => request?.resolve('cancel'), [request]);
  const confirm = useCallback(() => {
    if (!request || !canConfirm) return;
    if (dontAsk) saveDeleteDefault(scope);
    request.resolve(scope);
  }, [request, canConfirm, dontAsk, scope]);

  useEffect(() => {
    if (!request) return;
    const onKey = (e: KeyboardEvent) => {
      const handled = () => {
        e.preventDefault();
        e.stopPropagation();
      };
      if (e.key === 'Escape') {
        handled();
        close();
      } else if (e.key === 'Enter') {
        handled();
        confirm();
      } else if (['1', '2', '3'].includes(e.key) && choices[Number(e.key) - 1]) {
        handled();
        setScope(choices[Number(e.key) - 1]);
      }
    };
    // Capture phase: the library's own Delete/arrow shortcuts must not see these keys.
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [request, close, confirm, choices]);

  if (!request) return null;
  const count = request.photos.length;
  const unpaired = preview ? count - preview.pairs : 0;

  return createPortal(
    <div
      className="fixed inset-0 z-[9999] flex items-center justify-center bg-black/30 backdrop-blur-xs"
      role="dialog"
      aria-modal="true"
      aria-labelledby="pv-delete-title"
      onClick={close}
    >
      <div
        className="w-full max-w-md rounded-lg bg-surface p-6 text-text-primary shadow-xl"
        onClick={(e) => e.stopPropagation()}
      >
        <Text variant={TextVariants.title} id="pv-delete-title" className="mb-4">
          {t('pv.delete.title', { count })}
        </Text>

        {failed ? (
          <Text className="mb-4 text-red-400">{t('pv.delete.previewFailed')}</Text>
        ) : (
          <div className="flex flex-col gap-2" role="radiogroup">
            {choices.map((s, i) => {
              const option = optionOf(s);
              const empty = !!preview && (option?.files ?? 0) === 0;
              return (
                <button
                  key={s}
                  type="button"
                  role="radio"
                  aria-checked={scope === s}
                  disabled={empty}
                  onClick={() => setScope(s)}
                  className={clsx(
                    'flex items-start gap-3 rounded-md border px-3 py-2.5 text-left transition-colors',
                    scope === s
                      ? 'border-accent bg-accent/10'
                      : 'border-transparent bg-bg-primary hover:bg-bg-primary/70',
                    empty && 'cursor-not-allowed opacity-40',
                  )}
                >
                  <span
                    className={clsx(
                      'mt-1 h-3.5 w-3.5 shrink-0 rounded-full border-2',
                      scope === s ? 'border-accent bg-accent' : 'border-text-secondary',
                    )}
                  />
                  <span className="min-w-0 flex-1">
                    <span className="flex items-baseline justify-between gap-2">
                      <span className="font-semibold">{t(`pv.delete.scope.${s}`, { display: displayName })}</span>
                      <span className="shrink-0 text-xs text-text-secondary">{i + 1}</span>
                    </span>
                    <span className="block text-sm text-text-secondary">
                      {t(`pv.delete.scopeHint.${s}`, { display: displayName })}
                      {option && option.bytes > 0 && ` · ${formatBytes(option.bytes, t, 1)}`}
                    </span>
                    {option && option.locked > 0 && (
                      <span className="block text-sm text-amber-400">
                        {t('pv.delete.locked', { count: option.locked })}
                      </span>
                    )}
                  </span>
                </button>
              );
            })}
            {unpaired > 0 && choices.length > 1 && (
              <Text className="text-sm text-text-secondary">{t('pv.delete.unpaired', { count: unpaired })}</Text>
            )}
          </div>
        )}

        <label className="mt-4 flex cursor-pointer items-center gap-2 text-sm">
          <input
            type="checkbox"
            checked={dontAsk}
            onChange={(e) => setDontAsk(e.target.checked)}
            className="accent-accent"
          />
          {t('pv.delete.dontAsk')}
        </label>
        <Text className="mt-3 text-sm text-text-secondary">{t('pv.delete.recentlyDeleted')}</Text>

        <div className="mt-5 flex justify-end gap-3">
          <Button className="bg-bg-primary text-white shadow-none hover:bg-bg-primary" onClick={close} variant="ghost">
            {t('pv.delete.cancel')}
          </Button>
          <Button onClick={confirm} disabled={!canConfirm} variant="destructive" autoFocus>
            {t('pv.delete.confirm')}
          </Button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
