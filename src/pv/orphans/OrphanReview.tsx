// Pairview: orphan review (docs/IMPLEMENTATION_PLAN.md M2-05, PRD §6.6).
// A display copy deleted outside Pairview leaves a `⚠ RAW` Photo. A banner in the sidebar opens a
// review grid: Delete RAWs (to Recently Deleted) · Keep as RAW-only · Ignore. A lost display copy
// is a strong "drop" signal, so Photos start selected, except those locked Keep RAW (K).
// Nothing is ever deleted without the owner's click.
import { useCallback, useEffect, useMemo, useState } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import clsx from 'clsx';
import { toast } from 'react-toastify';
import { AlertTriangle, Check, Lock, Star, X } from 'lucide-react';
import Button from '../../components/ui/Button';
import Text from '../../components/ui/Text';
import { TextColors, TextVariants, TextWeights } from '../../types/typography';
import { formatBytes } from '../../utils/formatBytes';
import { useUIStore } from '../../store/useUIStore';
import { onFolderChanged, photoPreviewUrl, pvApi, type PhotoItem } from '../api';

const IGNORED_KEY = 'pv.orphans.ignored';

function loadIgnored(): Set<number> {
  try {
    return new Set(JSON.parse(localStorage.getItem(IGNORED_KEY) ?? '[]') as number[]);
  } catch {
    return new Set();
  }
}

function saveIgnored(ids: Set<number>) {
  try {
    localStorage.setItem(IGNORED_KEY, JSON.stringify([...ids]));
  } catch {
    // Private storage: the banner simply shows again.
  }
}

/** Sidebar banner, shown while there are orphans the owner hasn't ignored. */
export function OrphanBanner({ onChanged }: { onChanged(): void }) {
  const { t } = useTranslation();
  const [orphans, setOrphans] = useState<PhotoItem[]>([]);
  const [ignored, setIgnored] = useState<Set<number>>(loadIgnored);
  const [open, setOpen] = useState(false);

  const load = useCallback(async () => {
    try {
      setOrphans(await pvApi.orphans());
    } catch {
      setOrphans([]);
    }
  }, []);

  useEffect(() => {
    void load();
    const unlisten = onFolderChanged(() => void load());
    const onFocus = () => void load();
    window.addEventListener('focus', onFocus);
    return () => {
      void unlisten.then((stop) => stop());
      window.removeEventListener('focus', onFocus);
    };
  }, [load]);

  const fresh = orphans.filter((p) => !ignored.has(p.id));
  if (orphans.length === 0) return null;

  return (
    <>
      {fresh.length > 0 && (
        <Text as="div" color={TextColors.primary} weight={TextWeights.medium} className="pb-2">
          <div
            className="flex items-center gap-2 rounded-md bg-yellow-500/10 px-2 py-1.5 cursor-pointer hover:bg-yellow-500/20 transition-colors"
            onClick={() => setOpen(true)}
            title={t('pv.orphans.bannerHint')}
          >
            <AlertTriangle size={16} className="shrink-0 text-yellow-300" />
            <span className="min-w-0 flex-1 select-none truncate">
              {t('pv.orphans.banner', { count: fresh.length })}
            </span>
          </div>
        </Text>
      )}
      {open && (
        <OrphanReview
          orphans={orphans}
          onClose={() => setOpen(false)}
          onIgnore={(ids) => {
            const next = new Set([...ignored, ...ids]);
            saveIgnored(next);
            setIgnored(next);
            setOpen(false);
          }}
          onChanged={() => {
            onChanged();
            void load();
          }}
        />
      )}
    </>
  );
}

interface OrphanReviewProps {
  orphans: PhotoItem[];
  onClose(): void;
  onIgnore(ids: number[]): void;
  onChanged(): void;
}

function OrphanReview({ orphans, onClose, onIgnore, onChanged }: OrphanReviewProps) {
  const { t } = useTranslation();
  // A lost display copy is a strong "drop" signal: start with everything not locked selected.
  const [selected, setSelected] = useState<Set<number>>(
    () => new Set(orphans.filter((p) => p.keepRaw !== 'keep').map((p) => p.id)),
  );
  const [busy, setBusy] = useState(false);
  const chosen = useMemo(() => orphans.filter((p) => selected.has(p.id)), [orphans, selected]);
  const bytes = chosen.reduce((n, p) => n + p.size, 0);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !useUIStore.getState().confirmModalState?.isOpen) {
        e.preventDefault();
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [onClose]);

  const toggle = (id: number) =>
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  const deleteRaws = () => {
    const paths = chosen.map((p) => p.representativePath).filter((p): p is string => !!p);
    useUIStore.getState().setUI({
      confirmModalState: {
        isOpen: true,
        title: t('pv.orphans.deleteTitle', { count: paths.length }),
        message: t('pv.orphans.deleteMessage', { count: paths.length, size: formatBytes(bytes, t, 1) }),
        confirmText: t('pv.orphans.deleteConfirm'),
        confirmVariant: 'destructive',
        onConfirm: async () => {
          setBusy(true);
          try {
            const r = await pvApi.deletePhotos(paths, 'photo');
            toast.success(t('pv.delete.done', { count: r.photos, size: formatBytes(r.bytes, t, 1) }));
            onChanged();
            onClose();
          } catch (e) {
            toast.error(String(e));
          } finally {
            setBusy(false);
          }
        },
      },
    });
  };

  const keepRawOnly = async () => {
    setBusy(true);
    try {
      const n = await pvApi.keepRawOnly(chosen.map((p) => p.id));
      toast.success(t('pv.orphans.kept', { count: n }));
      onChanged();
      onClose();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  return createPortal(
    <div
      className="fixed inset-0 z-[9000] flex flex-col bg-bg-primary text-text-primary"
      role="dialog"
      aria-modal="true"
    >
      <header className="flex items-center gap-4 border-b border-surface px-6 py-4">
        <AlertTriangle size={20} className="text-yellow-300" />
        <div className="min-w-0 flex-1">
          <Text variant={TextVariants.title}>{t('pv.orphans.title', { count: orphans.length })}</Text>
          <Text className="text-sm text-text-secondary">{t('pv.orphans.subtitle')}</Text>
        </div>
        <button
          type="button"
          className="rounded-md p-2 text-text-secondary hover:bg-surface"
          onClick={onClose}
          aria-label={t('pv.orphans.close')}
        >
          <X size={20} />
        </button>
      </header>

      <div className="flex items-center gap-3 px-6 py-3">
        <Text className="flex-1 text-sm text-text-secondary">
          {t('pv.orphans.selected', { count: chosen.length, size: formatBytes(bytes, t, 1) })}
        </Text>
        <button
          type="button"
          className="text-sm text-text-secondary hover:text-text-primary"
          onClick={() => setSelected(new Set(orphans.map((p) => p.id)))}
        >
          {t('pv.orphans.selectAll')}
        </button>
        <button
          type="button"
          className="text-sm text-text-secondary hover:text-text-primary"
          onClick={() => setSelected(new Set())}
        >
          {t('pv.orphans.selectNone')}
        </button>
        <Button
          className="bg-surface text-text-primary"
          onClick={() => onIgnore(orphans.map((p) => p.id))}
          disabled={busy}
        >
          {t('pv.orphans.ignore')}
        </Button>
        <Button
          className="bg-surface text-text-primary"
          onClick={() => void keepRawOnly()}
          disabled={busy || chosen.length === 0}
        >
          {t('pv.orphans.keepRawOnly')}
        </Button>
        <Button onClick={deleteRaws} disabled={busy || chosen.length === 0} variant="destructive">
          {t('pv.orphans.deleteRaws', { count: chosen.length })}
        </Button>
      </div>

      <main className="flex-1 overflow-y-auto px-6 pb-6">
        <div className="grid grid-cols-[repeat(auto-fill,minmax(160px,1fr))] gap-3">
          {orphans.map((p) => (
            <OrphanTile key={p.id} photo={p} selected={selected.has(p.id)} onToggle={() => toggle(p.id)} />
          ))}
        </div>
      </main>
    </div>,
    document.body,
  );
}

function OrphanTile({ photo, selected, onToggle }: { photo: PhotoItem; selected: boolean; onToggle(): void }) {
  const { t } = useTranslation();
  const name = photo.representativePath?.split(/[\\/]/).pop() ?? photo.stem;
  return (
    <button
      type="button"
      onClick={onToggle}
      title={photo.representativePath ?? undefined}
      className={clsx(
        'relative flex flex-col overflow-hidden rounded-md bg-surface text-left ring-2 transition',
        selected ? 'ring-accent' : 'ring-transparent hover:ring-surface',
      )}
    >
      <div className="flex aspect-[3/2] items-center justify-center bg-black/30">
        <img src={photoPreviewUrl(photo.id)} alt="" loading="lazy" className="h-full w-full object-contain" />
      </div>
      <div className="flex items-center gap-1.5 px-2 py-1.5">
        <Text className="min-w-0 flex-1 truncate text-sm">{name}</Text>
        {photo.keepRaw === 'keep' && (
          <Lock size={12} className="shrink-0 text-accent" aria-label={t('pv.orphans.locked')} />
        )}
        {photo.rating > 0 && (
          <span className="flex shrink-0 items-center gap-0.5 text-xs text-text-secondary">
            <Star size={11} />
            {photo.rating}
          </span>
        )}
      </div>
      <Text className="px-2 pb-1.5 text-xs text-text-secondary">
        {photo.capturedAt?.replace('T', ' ').slice(0, 16)} · {formatBytes(photo.size, t, 1)}
      </Text>
      {selected && (
        <span className="absolute right-1.5 top-1.5 rounded-full bg-accent p-0.5 text-button-text">
          <Check size={14} />
        </span>
      )}
    </button>
  );
}
