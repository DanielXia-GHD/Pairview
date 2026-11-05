// Pairview: Ready to slim + Review for slimming (docs/IMPLEMENTATION_PLAN.md M2-09, PRD §6.5 Stage 4).
//
// Ready to slim (sidebar) lists shoots past the 30-day grace period. ⌘⇧K reviews the selection
// (or the whole view) at any time. The review sorts pairs into HIF is enough (pre-selected) ·
// Your call · Keep RAW with reason chips; nothing happens until Apply, and then only the selected
// RAWs go to Recently Deleted (undoable). There is no automatic or AI-only slimming.
import { useCallback, useEffect, useMemo, useState } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import clsx from 'clsx';
import { toast } from 'react-toastify';
import { Check, Lock, Scissors, X } from 'lucide-react';
import { create } from 'zustand';
import Button from '../../components/ui/Button';
import Text from '../../components/ui/Text';
import { TextColors, TextVariants, TextWeights } from '../../types/typography';
import { formatBytes } from '../../utils/formatBytes';
import { useUIStore } from '../../store/useUIStore';
import { useLibraryStore } from '../../store/useLibraryStore';
import {
  onFolderChanged,
  photoPreviewUrl,
  pvApi,
  type PhotoItem,
  type ReadyShoot,
  type SlimItem,
  type Verdict,
} from '../api';
import { pvPhotosFor } from '../delete/choose';
import { toastDeleted } from '../delete/undo';
import { usePvPhotoStore } from '../store';
import { usePvViewer } from '../viewer/PvViewer';
import { refuseIfOffline } from '../offline';

const VERDICTS: Verdict[] = ['hifEnough', 'yourCall', 'keepRaw'];

interface ReviewRequest {
  title: string;
  photos: PhotoItem[];
  /** Set when a whole shoot is reviewed from Ready to slim: Apply/Done marks it reviewed. */
  shootId?: number;
}

const useSlim = create<{ review: ReviewRequest | null; listOpen: boolean }>(() => ({
  review: null,
  listOpen: false,
}));

const isTyping = () => {
  const el = document.activeElement;
  return el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement || (el as HTMLElement)?.isContentEditable;
};

/** ⌘⇧K / Ctrl+Shift+K: Review for slimming on the selection, or on everything in view. */
export function onSlimKey(e: KeyboardEvent) {
  if (!(e.metaKey || e.ctrlKey) || !e.shiftKey || e.key.toLowerCase() !== 'k') return;
  if (useUIStore.getState().activeView !== 'library' || isTyping() || usePvViewer.getState().path) return;
  const selected = pvPhotosFor(useLibraryStore.getState().multiSelectedPaths);
  const photos = selected && selected.length > 1 ? selected : Object.values(usePvPhotoStore.getState().byPath);
  if (photos.length === 0) return;
  e.preventDefault();
  e.stopImmediatePropagation();
  if (refuseIfOffline(photos.map((p) => p.representativePath ?? p.displayPath))) return;
  useSlim.setState({ review: { title: '', photos } });
}

/** Sidebar entry: Ready to slim (shoot count). Also hosts the review screen. */
export function ReadyToSlimEntry({ onChanged }: { onChanged(): void }) {
  const { t } = useTranslation();
  const { review, listOpen } = useSlim();
  const [ready, setReady] = useState<ReadyShoot[]>([]);

  const load = useCallback(async () => {
    try {
      setReady(await pvApi.slimReady());
    } catch {
      setReady([]);
    }
  }, []);

  useEffect(() => {
    void load();
    const unlisten = onFolderChanged(() => void load());
    const onFocus = () => void load();
    window.addEventListener('focus', onFocus);
    window.addEventListener('keydown', onSlimKey, true);
    return () => {
      void unlisten.then((stop) => stop());
      window.removeEventListener('focus', onFocus);
      window.removeEventListener('keydown', onSlimKey, true);
    };
  }, [load]);

  const openShoot = async (shoot: ReadyShoot) => {
    try {
      const photos = await pvApi.listPhotos({ kind: 'folder', folderId: shoot.folderId, recursive: false });
      useSlim.setState({ listOpen: false, review: { title: shoot.name, photos, shootId: shoot.folderId } });
    } catch (e) {
      toast.error(String(e));
    }
  };

  return (
    <>
      {ready.length > 0 && (
        <Text as="div" color={TextColors.primary} weight={TextWeights.medium} className="pb-1">
          <div
            className="flex items-center gap-2 py-1.5 pl-1.5 pr-1.5 rounded-md cursor-pointer hover:bg-card-active transition-colors"
            onClick={() => useSlim.setState({ listOpen: true })}
          >
            <div className="w-5 h-5 flex items-center justify-center shrink-0 text-text-secondary">
              <Scissors size={16} />
            </div>
            <span className="min-w-0 flex-1 select-none truncate">{t('pv.slim.ready')}</span>
            <Text as="span" variant={TextVariants.small} color={TextColors.secondary} className="shrink-0 tabular-nums">
              {ready.length}
            </Text>
          </div>
        </Text>
      )}
      {listOpen && (
        <ReadyList
          ready={ready}
          onOpen={(s) => void openShoot(s)}
          onClose={() => useSlim.setState({ listOpen: false })}
        />
      )}
      {review && (
        <ReviewView
          request={review}
          onClose={() => useSlim.setState({ review: null })}
          onChanged={() => {
            onChanged();
            void load();
          }}
        />
      )}
    </>
  );
}

function useEscape(onClose: () => void) {
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
}

function ReadyList({ ready, onOpen, onClose }: { ready: ReadyShoot[]; onOpen(s: ReadyShoot): void; onClose(): void }) {
  const { t } = useTranslation();
  useEscape(onClose);
  const total = ready.reduce((n, s) => n + s.rawBytes, 0);
  return createPortal(
    <div
      className="fixed inset-0 z-[9000] flex flex-col bg-bg-primary text-text-primary"
      role="dialog"
      aria-modal="true"
    >
      <Header
        icon={<Scissors size={20} className="text-text-secondary" />}
        title={t('pv.slim.ready')}
        subtitle={t('pv.slim.readySubtitle', { size: formatBytes(total, t, 1) })}
        onClose={onClose}
      />
      <main className="flex-1 overflow-y-auto px-6 py-4">
        <div className="flex flex-col gap-2">
          {ready.map((s) => (
            <button
              key={s.folderId}
              type="button"
              onClick={() => onOpen(s)}
              className="flex items-center gap-4 rounded-md bg-surface px-4 py-3 text-left hover:bg-card-active"
              title={s.path}
            >
              <Text className="w-28 shrink-0 tabular-nums text-text-secondary">{s.date}</Text>
              <Text weight={TextWeights.semibold} className="min-w-0 flex-1 truncate">
                {s.name}
              </Text>
              <Text className="shrink-0 text-sm text-text-secondary">
                {t('pv.slim.pairs', { count: s.pairs })} · {t('pv.slim.upTo', { size: formatBytes(s.rawBytes, t, 1) })}
              </Text>
            </button>
          ))}
        </div>
      </main>
    </div>,
    document.body,
  );
}

function Header({
  icon,
  title,
  subtitle,
  onClose,
  children,
}: {
  icon: React.ReactNode;
  title: string;
  subtitle: string;
  onClose(): void;
  children?: React.ReactNode;
}) {
  const { t } = useTranslation();
  return (
    <header className="flex items-center gap-4 border-b border-surface px-6 py-4">
      {icon}
      <div className="min-w-0 flex-1">
        <Text variant={TextVariants.title} className="truncate">
          {title}
        </Text>
        <Text className="text-sm text-text-secondary">{subtitle}</Text>
      </div>
      {children}
      <button
        type="button"
        className="rounded-md p-2 text-text-secondary hover:bg-surface"
        onClick={onClose}
        aria-label={t('pv.slim.close')}
      >
        <X size={20} />
      </button>
    </header>
  );
}

function ReviewView({ request, onClose, onChanged }: { request: ReviewRequest; onClose(): void; onChanged(): void }) {
  const { t } = useTranslation();
  useEscape(onClose);
  const [items, setItems] = useState<SlimItem[] | null>(null);
  const [tab, setTab] = useState<Verdict>('hifEnough');
  const [drop, setDrop] = useState<Set<number>>(new Set());
  const [busy, setBusy] = useState(false);
  const byId = useMemo(() => new Map(request.photos.map((p) => [p.id, p])), [request.photos]);

  useEffect(() => {
    let cancelled = false;
    setItems(null);
    const args = request.shootId !== undefined ? { shootId: request.shootId } : { photoIds: [...byId.keys()] };
    pvApi
      .slimReview(args)
      .then((list) => {
        if (cancelled) return;
        setItems(list);
        // HIF is enough starts selected (a Photo locked Drop is in it; one locked Keep never is).
        setDrop(new Set(list.filter((i) => i.value.verdict === 'hifEnough').map((i) => i.value.photoId)));
      })
      .catch((e) => {
        if (cancelled) return;
        toast.error(String(e));
        setItems([]);
      });
    return () => {
      cancelled = true;
    };
  }, [request, byId]);

  const inTab = (v: Verdict) => (items ?? []).filter((i) => i.value.verdict === v);
  const chosen = (items ?? []).filter((i) => drop.has(i.value.photoId));
  const bytes = chosen.reduce((n, i) => n + i.rawBytes, 0);

  const toggle = (item: SlimItem) => {
    if (item.value.locked && item.value.verdict === 'keepRaw') return;
    setDrop((prev) => {
      const next = new Set(prev);
      if (next.has(item.value.photoId)) next.delete(item.value.photoId);
      else next.add(item.value.photoId);
      return next;
    });
  };

  const apply = () => {
    useUIStore.getState().setUI({
      confirmModalState: {
        isOpen: true,
        title: t('pv.slim.applyTitle', { count: chosen.length }),
        message: t('pv.slim.applyMessage', { count: chosen.length, size: formatBytes(bytes, t, 1) }),
        confirmText: t('pv.slim.applyConfirm'),
        confirmVariant: 'destructive',
        onConfirm: async () => {
          setBusy(true);
          try {
            const r = await pvApi.slimApply(
              chosen.map((i) => i.value.photoId),
              request.shootId !== undefined ? [request.shootId] : [],
            );
            toastDeleted(t('pv.slim.applied', { count: r.photos, size: formatBytes(r.bytes, t, 1) }));
            if (r.skipped.length > 0) {
              toast.info(t('pv.slim.skipped', { count: r.skipped.length }));
            }
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

  const done = async () => {
    if (request.shootId !== undefined) {
      try {
        await pvApi.slimMarkReviewed(request.shootId);
        onChanged();
      } catch (e) {
        toast.error(String(e));
        return;
      }
    }
    onClose();
  };

  const title = request.title || t('pv.slim.reviewTitle');
  return createPortal(
    <div
      className="fixed inset-0 z-[9000] flex flex-col bg-bg-primary text-text-primary"
      role="dialog"
      aria-modal="true"
    >
      <Header
        icon={<Scissors size={20} className="text-text-secondary" />}
        title={t('pv.slim.reviewOf', { name: title })}
        subtitle={t('pv.slim.reviewSubtitle')}
        onClose={onClose}
      >
        {request.shootId !== undefined && (
          <Button className="bg-surface text-text-primary" onClick={() => void done()} disabled={busy}>
            {t('pv.slim.done')}
          </Button>
        )}
        <Button onClick={apply} disabled={busy || chosen.length === 0} variant="destructive">
          {t('pv.slim.apply', { count: chosen.length, size: formatBytes(bytes, t, 1) })}
        </Button>
      </Header>

      <nav className="flex gap-2 px-6 pt-4">
        {VERDICTS.map((v) => (
          <button
            key={v}
            type="button"
            onClick={() => setTab(v)}
            className={clsx(
              'rounded-full px-3 py-1 text-sm',
              tab === v ? 'bg-accent text-button-text' : 'bg-surface text-text-secondary hover:text-text-primary',
            )}
          >
            {t(`pv.rawValue.verdict.${v}`)} · {inTab(v).length}
          </button>
        ))}
      </nav>

      <main className="flex-1 overflow-y-auto px-6 py-4">
        {items === null ? (
          <Text className="text-text-secondary">{t('pv.slim.analysing', { count: request.photos.length })}</Text>
        ) : inTab(tab).length === 0 ? (
          <Text className="text-text-secondary">{t('pv.slim.emptyTab')}</Text>
        ) : (
          <div className="grid grid-cols-[repeat(auto-fill,minmax(180px,1fr))] gap-3">
            {inTab(tab).map((item) => (
              <SlimTile
                key={item.value.photoId}
                item={item}
                photo={byId.get(item.value.photoId)}
                dropping={drop.has(item.value.photoId)}
                onToggle={() => toggle(item)}
              />
            ))}
          </div>
        )}
      </main>
    </div>,
    document.body,
  );
}

function SlimTile({
  item,
  photo,
  dropping,
  onToggle,
}: {
  item: SlimItem;
  photo?: PhotoItem;
  dropping: boolean;
  onToggle(): void;
}) {
  const { t } = useTranslation();
  const v = item.value;
  const name = photo?.representativePath?.split(/[\\/]/).pop() ?? photo?.stem ?? String(v.photoId);
  const lockedKeep = v.locked && v.verdict === 'keepRaw';
  return (
    <button
      type="button"
      onClick={onToggle}
      disabled={lockedKeep}
      className={clsx(
        'relative flex flex-col overflow-hidden rounded-md bg-surface text-left ring-2 transition',
        dropping ? 'ring-red-500' : 'ring-transparent hover:ring-surface',
        lockedKeep && 'cursor-not-allowed',
      )}
    >
      <div className="flex aspect-[3/2] items-center justify-center bg-black/30">
        <img src={photoPreviewUrl(v.photoId)} alt="" loading="lazy" className="h-full w-full object-contain" />
      </div>
      <div className="flex items-center gap-1.5 px-2 pt-1.5">
        <Text className="min-w-0 flex-1 truncate text-sm">{name}</Text>
        {v.locked && <Lock size={12} className="shrink-0 text-accent" aria-label={t('pv.rawValue.locked')} />}
        <span className="shrink-0 text-xs text-text-secondary">{formatBytes(item.rawBytes, t, 0)}</span>
      </div>
      <div className="flex flex-wrap gap-1 px-2 py-1.5">
        {v.chips.map((c) => (
          <span
            key={c.kind}
            className={clsx(
              'rounded-full px-1.5 py-0.5 text-[10px]',
              c.toward === 'keep' ? 'bg-emerald-500/15 text-emerald-300' : 'bg-bg-primary text-text-secondary',
            )}
          >
            {t(`pv.rawValue.chip.${c.kind}`, { value: c.value })}
          </span>
        ))}
        {v.unconfirmed && (
          <span className="rounded-full bg-amber-500/15 px-1.5 py-0.5 text-[10px] text-amber-300">
            {t('pv.slim.suggestionOnly')}
          </span>
        )}
      </div>
      {dropping && (
        <span className="absolute right-1.5 top-1.5 flex items-center gap-1 rounded-full bg-red-600 px-1.5 py-0.5 text-[10px] font-semibold text-white">
          <Check size={11} />
          {t('pv.slim.dropBadge')}
        </span>
      )}
    </button>
  );
}
