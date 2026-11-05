// Pairview: Recently Deleted (docs/IMPLEMENTATION_PLAN.md M2-04, PRD §6.6).
// Grouped by reason (Deleted · Slimmed RAWs · Replaced originals), newest first. Restore an
// operation or chosen Photos; Empty now deletes for good, after a confirmation.
import { useCallback, useEffect, useMemo, useState } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import clsx from 'clsx';
import { toast } from 'react-toastify';
import { Check, FileText, RotateCcw, Trash2, X } from 'lucide-react';
import { create } from 'zustand';
import Button from '../../components/ui/Button';
import Text from '../../components/ui/Text';
import { TextColors, TextVariants, TextWeights } from '../../types/typography';
import { formatBytes } from '../../utils/formatBytes';
import { useUIStore } from '../../store/useUIStore';
import {
  deletedPreviewUrl,
  onFolderChanged,
  pvApi,
  type DeletedLocation,
  type DeletedOp,
  type DeletedPhoto,
  type DeleteReason,
} from '../api';

const REASONS: DeleteReason[] = ['deleted', 'slimmed', 'replaced'];
const DAY_MS = 24 * 60 * 60 * 1000;

const useRecentlyDeletedView = create<{ open: boolean }>(() => ({ open: false }));

/** Sidebar entry: "Recently Deleted" with its Photo count. Opens the view. */
export function RecentlyDeletedEntry({ onRestored }: { onRestored(): void }) {
  const { t } = useTranslation();
  const open = useRecentlyDeletedView((s) => s.open);
  const [count, setCount] = useState(0);

  const load = useCallback(async () => {
    try {
      const all = await pvApi.recentlyDeleted();
      setCount(all.flatMap((l) => l.ops).reduce((n, op) => n + op.photos.length + op.looseFiles.length, 0));
    } catch {
      setCount(0);
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

  return (
    <>
      <Text as="div" color={TextColors.primary} weight={TextWeights.medium} className="pb-2">
        <div
          className="flex items-center gap-2 py-1.5 pl-1.5 pr-1.5 rounded-md cursor-pointer hover:bg-card-active transition-colors"
          onClick={() => useRecentlyDeletedView.setState({ open: true })}
        >
          <div className="w-5 h-5 flex items-center justify-center shrink-0 text-text-secondary">
            <Trash2 size={16} />
          </div>
          <span className="min-w-0 flex-1 select-none truncate">{t('pv.deleted.title')}</span>
          {count > 0 && (
            <Text as="span" variant={TextVariants.small} color={TextColors.secondary} className="shrink-0 tabular-nums">
              {count}
            </Text>
          )}
        </div>
      </Text>
      {open && (
        <RecentlyDeletedView
          onClose={() => {
            useRecentlyDeletedView.setState({ open: false });
            void load();
          }}
          onRestored={onRestored}
        />
      )}
    </>
  );
}

interface Selection {
  locationId: number;
  opId: string;
  photos: Set<number>;
}

function RecentlyDeletedView({ onClose, onRestored }: { onClose(): void; onRestored(): void }) {
  const { t } = useTranslation();
  const [locations, setLocations] = useState<DeletedLocation[] | null>(null);
  const [reason, setReason] = useState<DeleteReason>('deleted');
  const [selection, setSelection] = useState<Selection | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      setLocations(await pvApi.recentlyDeleted());
    } catch (e) {
      toast.error(String(e));
      setLocations([]);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

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

  const ops = useMemo(() => (locations ?? []).flatMap((l) => l.ops.map((op) => ({ location: l, op }))), [locations]);
  const byReason = (r: DeleteReason) => ops.filter((o) => o.op.reason === r);
  const reasons = REASONS.filter((r) => byReason(r).length > 0);
  const shown = reasons.includes(reason) ? reason : (reasons[0] ?? 'deleted');
  const totalBytes = ops.reduce((n, o) => n + o.op.bytes, 0);
  const offline = (locations ?? []).filter((l) => l.status !== 'online');

  const restore = async (locationId: number, op: DeletedOp, photos?: number[]) => {
    setBusy(true);
    try {
      const r = await pvApi.restoreDeleted(locationId, op.opId, photos, photos ? [] : undefined);
      toast.success(t('pv.deleted.restored', { count: r.files }));
      if (r.conflicts.length > 0) toast.warn(t('pv.deleted.conflicts', { count: r.conflicts.length }));
      setSelection(null);
      onRestored();
      await load();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  const emptyNow = () => {
    const files = ops.reduce(
      (n, o) => n + o.op.photos.reduce((m, p) => m + p.files.length, 0) + o.op.looseFiles.length,
      0,
    );
    useUIStore.getState().setUI({
      confirmModalState: {
        isOpen: true,
        title: t('pv.deleted.emptyTitle'),
        message: t('pv.deleted.emptyMessage', { count: files, size: formatBytes(totalBytes, t, 1) }),
        confirmText: t('pv.deleted.emptyConfirm'),
        confirmVariant: 'destructive',
        onConfirm: async () => {
          setBusy(true);
          try {
            const p = await pvApi.emptyDeleted();
            toast.success(t('pv.deleted.emptied', { count: p.files, size: formatBytes(p.bytes, t, 1) }));
            await load();
          } catch (e) {
            toast.error(String(e));
          } finally {
            setBusy(false);
          }
        },
      },
    });
  };

  const toggle = (locationId: number, opId: string, index: number) =>
    setSelection((prev) => {
      const same = prev && prev.locationId === locationId && prev.opId === opId;
      const photos = new Set(same ? prev.photos : []);
      if (photos.has(index)) photos.delete(index);
      else photos.add(index);
      return photos.size > 0 ? { locationId, opId, photos } : null;
    });

  return createPortal(
    <div
      className="fixed inset-0 z-[9000] flex flex-col bg-bg-primary text-text-primary"
      role="dialog"
      aria-modal="true"
    >
      <header className="flex items-center gap-4 border-b border-surface px-6 py-4">
        <Trash2 size={20} className="text-text-secondary" />
        <div className="min-w-0 flex-1">
          <Text variant={TextVariants.title}>{t('pv.deleted.title')}</Text>
          <Text className="text-sm text-text-secondary">
            {t('pv.deleted.subtitle', { size: formatBytes(totalBytes, t, 1) })}
          </Text>
        </div>
        <Button onClick={emptyNow} disabled={busy || ops.length === 0} variant="destructive">
          {t('pv.deleted.emptyNow')}
        </Button>
        <button
          type="button"
          className="rounded-md p-2 text-text-secondary hover:bg-surface"
          onClick={onClose}
          aria-label={t('pv.deleted.close')}
        >
          <X size={20} />
        </button>
      </header>

      {reasons.length > 1 && (
        <nav className="flex gap-2 px-6 pt-4">
          {reasons.map((r) => (
            <button
              key={r}
              type="button"
              onClick={() => setReason(r)}
              className={clsx(
                'rounded-full px-3 py-1 text-sm',
                shown === r ? 'bg-accent text-button-text' : 'bg-surface text-text-secondary hover:text-text-primary',
              )}
            >
              {t(`pv.deleted.reason.${r}`)} ·{' '}
              {formatBytes(
                byReason(r).reduce((n, o) => n + o.op.bytes, 0),
                t,
                1,
              )}
            </button>
          ))}
        </nav>
      )}

      <main className="flex-1 overflow-y-auto px-6 py-4">
        {locations === null && <Text className="text-text-secondary">{t('pv.deleted.loading')}</Text>}
        {locations !== null && ops.length === 0 && (
          <div className="flex h-full flex-col items-center justify-center gap-2 text-text-secondary">
            <Trash2 size={40} />
            <Text>{t('pv.deleted.empty')}</Text>
          </div>
        )}
        {offline.map((l) => (
          <Text key={l.locationId} className="mb-3 text-sm text-amber-400">
            {t('pv.deleted.offline', { name: l.name })}
          </Text>
        ))}
        {byReason(shown).map(({ location, op }) => (
          <OpSection
            key={`${location.locationId}/${op.opId}`}
            location={location}
            op={op}
            selected={
              selection?.opId === op.opId && selection.locationId === location.locationId ? selection.photos : null
            }
            busy={busy}
            onToggle={(index) => toggle(location.locationId, op.opId, index)}
            onRestore={(photos) => void restore(location.locationId, op, photos)}
          />
        ))}
      </main>
    </div>,
    document.body,
  );
}

interface OpSectionProps {
  location: DeletedLocation;
  op: DeletedOp;
  selected: Set<number> | null;
  busy: boolean;
  onToggle(index: number): void;
  onRestore(photos?: number[]): void;
}

function OpSection({ location, op, selected, busy, onToggle, onRestore }: OpSectionProps) {
  const { t, i18n } = useTranslation();
  const when = new Date(op.createdAt).toLocaleString(i18n.language, { dateStyle: 'medium', timeStyle: 'short' });
  const daysLeft = op.expiresAt ? Math.max(0, Math.ceil((op.expiresAt - Date.now()) / DAY_MS)) : null;
  const items = op.photos.length + op.looseFiles.length;

  return (
    <section className="mb-8">
      <div className="mb-3 flex items-center gap-3">
        <div className="min-w-0 flex-1">
          <Text weight={TextWeights.semibold}>
            {when} · {t('pv.deleted.items', { count: items })} · {formatBytes(op.bytes, t, 1)}
          </Text>
          <Text className="text-sm text-text-secondary">
            {location.name}
            {daysLeft !== null && ` · ${t('pv.deleted.daysLeft', { count: daysLeft })}`}
          </Text>
        </div>
        {selected && selected.size > 0 ? (
          <Button onClick={() => onRestore([...selected])} disabled={busy}>
            <RotateCcw size={16} />
            {t('pv.deleted.restoreSelected', { count: selected.size })}
          </Button>
        ) : (
          <Button onClick={() => onRestore()} disabled={busy}>
            <RotateCcw size={16} />
            {t('pv.deleted.restoreAll')}
          </Button>
        )}
      </div>
      <div className="grid grid-cols-[repeat(auto-fill,minmax(150px,1fr))] gap-3">
        {op.photos.map((p) => (
          <PhotoTile
            key={p.index}
            locationId={location.locationId}
            opId={op.opId}
            photo={p}
            selected={!!selected?.has(p.index)}
            onToggle={() => onToggle(p.index)}
          />
        ))}
      </div>
      {op.looseFiles.length > 0 && (
        <ul className="mt-3 space-y-1 text-sm text-text-secondary">
          {op.looseFiles.map((f) => (
            <li key={f} className="flex items-center gap-2 truncate">
              <FileText size={14} className="shrink-0" />
              <span className="truncate">{f}</span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function PhotoTile({
  locationId,
  opId,
  photo,
  selected,
  onToggle,
}: {
  locationId: number;
  opId: string;
  photo: DeletedPhoto;
  selected: boolean;
  onToggle(): void;
}) {
  const { t } = useTranslation();
  const [failed, setFailed] = useState(false);
  return (
    <button
      type="button"
      onClick={onToggle}
      title={photo.files.join('\n')}
      className={clsx(
        'group relative flex flex-col overflow-hidden rounded-md bg-surface text-left ring-2 transition',
        selected ? 'ring-accent' : 'ring-transparent hover:ring-surface',
      )}
    >
      <div className="flex aspect-[3/2] items-center justify-center bg-black/30">
        {failed ? (
          <FileText size={28} className="text-text-secondary" />
        ) : (
          <img
            src={deletedPreviewUrl(locationId, opId, photo.index)}
            alt=""
            loading="lazy"
            className="h-full w-full object-contain"
            onError={() => setFailed(true)}
          />
        )}
      </div>
      <div className="px-2 py-1.5">
        <Text className="truncate text-sm">{photo.files.find((f) => !/\.(xmp|rrdata)$/i.test(f)) ?? photo.stem}</Text>
        <Text className="text-xs text-text-secondary">
          {t('pv.deleted.files', { count: photo.files.length })} · {formatBytes(photo.bytes, t, 1)}
        </Text>
      </div>
      {selected && (
        <span className="absolute right-1.5 top-1.5 rounded-full bg-accent p-0.5 text-button-text">
          <Check size={14} />
        </span>
      )}
    </button>
  );
}
