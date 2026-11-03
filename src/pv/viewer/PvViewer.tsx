// Pairview: Pixea-style viewer in the main window (PRD §6.2, docs/IMPLEMENTATION_PLAN.md M1-14).
// Shows a Photo's display copy (A5) in grid order, with prefetch, zoom/pan, full screen and
// Photo-level rating keys. While open it takes the keyboard, so library shortcuts don't fire.
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { getCurrentWindow } from '@tauri-apps/api/window';
import clsx from 'clsx';
import { ChevronLeft, ChevronRight, Expand, Lock, Minimize, Pencil, Star, X } from 'lucide-react';
import { create } from 'zustand';
import { useLibraryStore } from '../../store/useLibraryStore';
import { useUIStore } from '../../store/useUIStore';
import { COLOR_LABELS } from '../../utils/adjustments';
import type { ImageFile } from '../../components/ui/AppProperties';
import { pvApi, type ColorLabel, type KeepRaw, type MetaChange, type PhotoItem } from '../api';
import { basePath, pvPhotoForPath, usePvPhotoStore } from '../store';
import { badgeLabel } from '../grid/PvTileBadge';

const PREFETCH_AHEAD = 2;
const PREFETCH_BEHIND = 1;
const CHROME_HIDE_MS = 2500;
const SWIPE_THRESHOLD = 90;
const LABEL_KEYS: Record<string, ColorLabel> = { '6': 'red', '7': 'yellow', '8': 'green', '9': 'blue' };
const KEEP_CYCLE: KeepRaw[] = ['auto', 'keep', 'drop'];

// ---- Open / close ----

interface ViewerState {
  path: string | null;
  open(path: string): boolean;
  close(): void;
}

export const usePvViewer = create<ViewerState>((set) => ({
  path: null,
  /** Opens the viewer if `path` is a Pairview Photo; returns false otherwise. */
  open: (path) => {
    if (!pvPhotoForPath(path)) return false;
    set({ path });
    return true;
  },
  close: () => set({ path: null }),
}));

export const pvOpenViewer = (path: string) => usePvViewer.getState().open(path);

function imageUrl(photo: PhotoItem, jpeg: boolean) {
  const base = navigator.userAgent.includes('Windows') ? 'http://pvimg.localhost' : 'pvimg://localhost';
  return `${base}/photo/${photo.id}${jpeg ? '?jpeg' : ''}`;
}

const isTyping = () => {
  const el = document.activeElement;
  return el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement || (el as HTMLElement)?.isContentEditable;
};

// ---- Viewer ----

interface PvViewerProps {
  /** The grid's current list (sorted and filtered): the viewer steps through it. */
  sortedImageList: ImageFile[];
  onOpenEditor(path: string): void;
}

export default function PvViewer({ sortedImageList, onOpenEditor }: PvViewerProps) {
  const path = usePvViewer((s) => s.path);

  // Space opens the viewer on the active library tile.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (usePvViewer.getState().path || e.code !== 'Space' || e.metaKey || e.ctrlKey || e.altKey || isTyping()) return;
      if (useUIStore.getState().activeView !== 'library') return;
      const active = useLibraryStore.getState().libraryActivePath;
      if (active && pvOpenViewer(active)) {
        e.preventDefault();
        e.stopPropagation();
      }
    };
    window.addEventListener('keydown', onKey, { capture: true });
    return () => window.removeEventListener('keydown', onKey, { capture: true });
  }, []);

  if (!path) return null;
  return <ViewerOverlay key="viewer" startPath={path} list={sortedImageList} onOpenEditor={onOpenEditor} />;
}

interface OverlayProps {
  startPath: string;
  list: ImageFile[];
  onOpenEditor(path: string): void;
}

function ViewerOverlay({ startPath, list, onOpenEditor }: OverlayProps) {
  const { t } = useTranslation();
  const byPath = usePvPhotoStore((s) => s.byPath);
  const paths = useMemo(() => list.map((f) => f.path).filter((p) => byPath[basePath(p)]), [list, byPath]);
  const [current, setCurrent] = useState(startPath);
  const index = Math.max(0, paths.indexOf(current));
  const photo = byPath[basePath(paths[index] ?? current)];

  const [chrome, setChrome] = useState(true);
  const hideTimer = useRef<number | undefined>(undefined);
  const [fullscreen, setFullscreen] = useState(false);
  const enteredFullscreen = useRef(false);
  const [error, setError] = useState<string | null>(null);

  const go = useCallback(
    (by: number) => {
      const next = paths[index + by];
      if (next) setCurrent(next);
    },
    [paths, index],
  );

  const close = useCallback(() => {
    const { setLibrary } = useLibraryStore.getState();
    // Back to the grid on the Photo last viewed.
    setLibrary({ libraryActivePath: current, multiSelectedPaths: [current] });
    if (enteredFullscreen.current) void getCurrentWindow().setFullscreen(false);
    usePvViewer.getState().close();
  }, [current]);

  const toggleFullscreen = useCallback(async () => {
    const win = getCurrentWindow();
    const next = !(await win.isFullscreen());
    await win.setFullscreen(next);
    enteredFullscreen.current = next;
    setFullscreen(next);
  }, []);

  const showChrome = useCallback(() => {
    setChrome(true);
    window.clearTimeout(hideTimer.current);
    hideTimer.current = window.setTimeout(() => setChrome(false), CHROME_HIDE_MS);
  }, []);
  useEffect(() => {
    showChrome();
    return () => window.clearTimeout(hideTimer.current);
  }, [showChrome]);

  /** Writes a change for the Photo and updates the grid's copy of it. */
  const apply = useCallback(
    async (change: MetaChange) => {
      if (!photo) return;
      setError(null);
      try {
        const [result] = await pvApi.setMetadata([photo.id], change);
        if (result?.error) {
          setError(result.error);
          return;
        }
        const meta = result?.meta;
        if (!meta) return;
        const updated = { ...photo, ...meta };
        usePvPhotoStore.setState((s) => {
          const key = Object.keys(s.byPath).find((k) => s.byPath[k].id === photo.id);
          return key ? { byPath: { ...s.byPath, [key]: updated } } : s;
        });
        const gridPath = photo.representativePath ?? photo.displayPath;
        useLibraryStore.getState().setLibrary((s) => ({
          imageRatings: gridPath ? { ...s.imageRatings, [gridPath]: meta.rating } : s.imageRatings,
          imageList: s.imageList.map((f) =>
            basePath(f.path) === gridPath
              ? {
                  ...f,
                  rating: meta.rating,
                  flag: meta.flag === 'none' ? null : (meta.flag as ImageFile['flag']),
                  tags: [
                    ...(f.tags ?? []).filter((tag) => !tag.startsWith('color:')),
                    ...(meta.label ? [`color:${meta.label}`] : []),
                  ],
                }
              : f,
          ),
        }));
      } catch (e) {
        setError(String(e));
      }
    },
    [photo],
  );

  // Keyboard: captured before the library's own shortcuts.
  const zoomRef = useRef<ZoomApi | null>(null);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (isTyping()) return;
      e.stopPropagation();
      if (e.metaKey || e.ctrlKey) return; // Leave app shortcuts (⌘Q, …) alone.
      const key = e.key;
      let handled = true;
      if (key === 'ArrowRight' || key === 'ArrowDown' || key === 'PageDown') go(1);
      else if (key === 'ArrowLeft' || key === 'ArrowUp' || key === 'PageUp') go(-1);
      else if (key === 'Home') setCurrent(paths[0] ?? current);
      else if (key === 'End') setCurrent(paths[paths.length - 1] ?? current);
      else if (key === 'Escape' || key === ' ') close();
      else if (key === 'f' || key === 'F') void toggleFullscreen();
      else if (key === 'e' || key === 'E') {
        const target = photo?.representativePath ?? photo?.displayPath;
        close();
        if (target) onOpenEditor(target);
      } else if (/^[0-5]$/.test(key)) void apply({ rating: Number(key) });
      else if (LABEL_KEYS[key]) void apply({ label: photo?.label === LABEL_KEYS[key] ? '' : LABEL_KEYS[key] });
      else if (key === 'p' || key === 'P') void apply({ flag: photo?.flag === 'pick' ? 'none' : 'pick' });
      else if (key === 'x' || key === 'X') void apply({ flag: photo?.flag === 'reject' ? 'none' : 'reject' });
      else if (key === 'u' || key === 'U') void apply({ flag: 'none' });
      else if (key === 'k' || key === 'K') {
        const next = KEEP_CYCLE[(KEEP_CYCLE.indexOf(photo?.keepRaw ?? 'auto') + 1) % KEEP_CYCLE.length];
        void apply({ keepRaw: next });
      } else if (key === 'z' || key === 'Z') zoomRef.current?.toggleActual();
      else if (key === '\\') zoomRef.current?.fit();
      else if (key === '+' || key === '=') zoomRef.current?.zoomBy(1.25);
      else if (key === '-' || key === '_') zoomRef.current?.zoomBy(0.8);
      else handled = false;
      if (handled) e.preventDefault();
      showChrome();
    };
    window.addEventListener('keydown', onKey, { capture: true });
    return () => window.removeEventListener('keydown', onKey, { capture: true });
  }, [apply, close, current, go, onOpenEditor, paths, photo, showChrome, toggleFullscreen]);

  // Prefetch neighbours so ←/→ is instant.
  useEffect(() => {
    const neighbours = [
      ...paths.slice(index + 1, index + 1 + PREFETCH_AHEAD),
      ...paths.slice(Math.max(0, index - PREFETCH_BEHIND), index),
    ];
    const images = neighbours
      .map((p) => byPath[basePath(p)])
      .filter(Boolean)
      .map((p) => {
        const img = new Image();
        img.src = imageUrl(p, false);
        img.decode().catch(() => {});
        return img;
      });
    return () => images.forEach((img) => (img.src = ''));
  }, [paths, index, byPath]);

  if (!photo) return null;
  const color = COLOR_LABELS.find((c) => c.name === photo.label)?.color;
  const name = (photo.displayPath ?? photo.representativePath ?? '').split(/[\\/]/).pop();

  return (
    <div
      className={clsx('fixed inset-0 z-50 bg-black select-none', !chrome && 'cursor-none')}
      onMouseMove={showChrome}
      role="dialog"
      aria-label={t('pv.viewer.title')}
    >
      <ZoomableImage key={photo.id} photo={photo} apiRef={zoomRef} onSwipe={go} />

      {/* Top bar */}
      <div
        className={clsx(
          'absolute inset-x-0 top-0 flex items-center gap-3 bg-linear-to-b from-black/70 to-transparent px-4 pt-3 pb-8 text-white transition-opacity duration-300',
          chrome ? 'opacity-100' : 'opacity-0 pointer-events-none',
        )}
        onMouseEnter={() => window.clearTimeout(hideTimer.current)}
      >
        <ChromeButton title={t('pv.viewer.close')} onClick={close}>
          <X size={18} />
        </ChromeButton>
        <div className="min-w-0 flex-1">
          <div className="truncate text-sm font-semibold">{name}</div>
          <div className="text-xs text-white/70">
            {index + 1} / {paths.length}
            {photo.capturedAt && ` · ${photo.capturedAt.replace('T', ' ').slice(0, 19)}`}
            {photo.camera && ` · ${photo.camera}`}
          </div>
        </div>
        <span className="rounded-full bg-white/15 px-2 py-0.5 text-[11px] font-semibold">{badgeLabel(photo)}</span>
        <ChromeButton
          title={t('pv.viewer.edit')}
          onClick={() => {
            const target = photo.representativePath ?? photo.displayPath;
            close();
            if (target) onOpenEditor(target);
          }}
        >
          <Pencil size={16} />
        </ChromeButton>
        <ChromeButton title={t('pv.viewer.fullscreen')} onClick={() => void toggleFullscreen()}>
          {fullscreen ? <Minimize size={16} /> : <Expand size={16} />}
        </ChromeButton>
      </div>

      {/* Bottom bar: Photo metadata */}
      <div
        className={clsx(
          'absolute inset-x-0 bottom-0 flex items-center justify-center gap-4 bg-linear-to-t from-black/70 to-transparent px-4 pt-8 pb-3 text-white transition-opacity duration-300',
          chrome ? 'opacity-100' : 'opacity-0 pointer-events-none',
        )}
      >
        <div className="flex items-center gap-0.5" aria-label={t('pv.viewer.rating', { count: photo.rating })}>
          {[1, 2, 3, 4, 5].map((n) => (
            <button key={n} type="button" onClick={() => void apply({ rating: photo.rating === n ? 0 : n })}>
              <Star size={16} className={n <= photo.rating ? 'fill-white text-white' : 'text-white/40'} />
            </button>
          ))}
        </div>
        {photo.flag !== 'none' && (
          <span className={clsx('text-xs font-semibold', photo.flag === 'pick' ? 'text-green-400' : 'text-red-400')}>
            {t(`pv.viewer.flag.${photo.flag}`)}
          </span>
        )}
        {color && <span className="h-3 w-3 rounded-full" style={{ backgroundColor: color }} />}
        {photo.keepRaw !== 'auto' && (
          <span className="flex items-center gap-1 text-xs">
            <Lock size={12} />
            {t(`pv.viewer.keepRaw.${photo.keepRaw}`)}
          </span>
        )}
        {error && <span className="text-xs text-red-400">{error}</span>}
        <span className="absolute right-4 text-[11px] text-white/50">{t('pv.viewer.keysHint')}</span>
      </div>

      {/* Side arrows */}
      {index > 0 && <SideArrow side="left" visible={chrome} onClick={() => go(-1)} title={t('pv.viewer.previous')} />}
      {index < paths.length - 1 && (
        <SideArrow side="right" visible={chrome} onClick={() => go(1)} title={t('pv.viewer.next')} />
      )}
    </div>
  );
}

// ---- Zoomable image ----

interface ZoomApi {
  fit(): void;
  toggleActual(): void;
  zoomBy(factor: number): void;
}

interface ZoomableImageProps {
  photo: PhotoItem;
  apiRef: React.MutableRefObject<ZoomApi | null>;
  onSwipe(by: number): void;
}

/** `scale: null` = fit to the window; otherwise 1 = 100% (one image pixel per screen pixel). */
function ZoomableImage({ photo, apiRef, onSwipe }: ZoomableImageProps) {
  const { t } = useTranslation();
  const box = useRef<HTMLDivElement>(null);
  const [jpeg, setJpeg] = useState(false);
  const [failed, setFailed] = useState(false);
  const [natural, setNatural] = useState<{ w: number; h: number } | null>(null);
  const [scale, setScale] = useState<number | null>(null);
  const [offset, setOffset] = useState({ x: 0, y: 0 });
  const drag = useRef<{ x: number; y: number; ox: number; oy: number; moved: boolean } | null>(null);
  const swipe = useRef({ sum: 0, at: 0, lockedUntil: 0 });
  const dpr = window.devicePixelRatio || 1;

  const fitScale = useCallback(() => {
    const el = box.current;
    if (!el || !natural) return 1;
    return Math.min(el.clientWidth / natural.w, el.clientHeight / natural.h, 1 / dpr) * dpr;
  }, [natural, dpr]);

  /** Sets an absolute zoom, keeping the image point under (cx, cy) in place. */
  const zoomTo = useCallback(
    (next: number, cx?: number, cy?: number) => {
      const el = box.current;
      if (!el || !natural) return;
      const current = scale ?? fitScale();
      const clamped = Math.min(Math.max(next, fitScale()), 8);
      if (clamped <= fitScale() + 1e-3) {
        setScale(null);
        setOffset({ x: 0, y: 0 });
        return;
      }
      const px = (cx ?? el.clientWidth / 2) - el.clientWidth / 2;
      const py = (cy ?? el.clientHeight / 2) - el.clientHeight / 2;
      const ratio = clamped / current;
      setOffset((o) => ({ x: px - (px - o.x) * ratio, y: py - (py - o.y) * ratio }));
      setScale(clamped);
    },
    [natural, scale, fitScale],
  );

  useEffect(() => {
    apiRef.current = {
      fit: () => {
        setScale(null);
        setOffset({ x: 0, y: 0 });
      },
      toggleActual: () => (scale === null ? zoomTo(1) : (setScale(null), setOffset({ x: 0, y: 0 }))),
      zoomBy: (f) => zoomTo((scale ?? fitScale()) * f),
    };
  }, [apiRef, scale, zoomTo, fitScale]);

  const onWheel = (e: React.WheelEvent) => {
    const rect = box.current?.getBoundingClientRect();
    if (e.ctrlKey || scale !== null) {
      // Pinch (ctrlKey) or scroll while zoomed: zoom around the pointer.
      if (e.ctrlKey || !e.shiftKey) {
        const factor = Math.exp(-e.deltaY * (e.ctrlKey ? 0.01 : 0.002));
        zoomTo((scale ?? fitScale()) * factor, e.clientX - (rect?.left ?? 0), e.clientY - (rect?.top ?? 0));
      }
      return;
    }
    // Two-finger horizontal swipe: next / previous Photo.
    const now = performance.now();
    const s = swipe.current;
    if (now < s.lockedUntil) return; // The rest of a gesture that already turned the page.
    if (now - s.at > 300) s.sum = 0;
    s.at = now;
    s.sum += e.deltaX;
    if (Math.abs(s.sum) > SWIPE_THRESHOLD) {
      onSwipe(s.sum > 0 ? 1 : -1);
      s.sum = 0;
      s.lockedUntil = now + 500;
    }
  };

  const shown = scale ?? fitScale();
  const style: React.CSSProperties = natural
    ? {
        width: (natural.w * shown) / dpr,
        height: (natural.h * shown) / dpr,
        transform: `translate(calc(-50% + ${offset.x}px), calc(-50% + ${offset.y}px))`,
      }
    : { maxWidth: '100%', maxHeight: '100%', transform: 'translate(-50%, -50%)' };

  return (
    <div
      ref={box}
      className={clsx('absolute inset-0 overflow-hidden', scale !== null ? 'cursor-grab' : 'cursor-zoom-in')}
      onWheel={onWheel}
      onMouseDown={(e) => {
        if (e.button !== 0) return;
        drag.current = { x: e.clientX, y: e.clientY, ox: offset.x, oy: offset.y, moved: false };
      }}
      onMouseMove={(e) => {
        const d = drag.current;
        if (!d || scale === null) return;
        if (Math.abs(e.clientX - d.x) + Math.abs(e.clientY - d.y) > 3) d.moved = true;
        setOffset({ x: d.ox + e.clientX - d.x, y: d.oy + e.clientY - d.y });
      }}
      onMouseUp={(e) => {
        const d = drag.current;
        drag.current = null;
        if (d?.moved) return;
        // Click: fit ↔ 100% at the clicked point.
        const rect = box.current?.getBoundingClientRect();
        if (scale === null) zoomTo(1, e.clientX - (rect?.left ?? 0), e.clientY - (rect?.top ?? 0));
        else {
          setScale(null);
          setOffset({ x: 0, y: 0 });
        }
      }}
      onMouseLeave={() => (drag.current = null)}
    >
      {failed ? (
        <div className="absolute inset-0 flex items-center justify-center text-sm text-white/70">
          {t('pv.viewer.cannotShow')}
        </div>
      ) : (
        <img
          src={imageUrl(photo, jpeg)}
          alt=""
          draggable={false}
          className="absolute left-1/2 top-1/2 max-w-none"
          style={style}
          onLoad={(e) => setNatural({ w: e.currentTarget.naturalWidth, h: e.currentTarget.naturalHeight })}
          onError={() => (jpeg ? setFailed(true) : setJpeg(true))}
        />
      )}
      {scale !== null && (
        <div className="absolute left-4 top-20 rounded bg-black/50 px-2 py-0.5 text-xs text-white">
          {Math.round(scale * 100)}%
        </div>
      )}
    </div>
  );
}

// ---- Chrome bits ----

function ChromeButton({ children, title, onClick }: { children: React.ReactNode; title: string; onClick(): void }) {
  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      onClick={onClick}
      className="rounded-full p-2 text-white/90 hover:bg-white/15"
    >
      {children}
    </button>
  );
}

function SideArrow({
  side,
  visible,
  onClick,
  title,
}: {
  side: 'left' | 'right';
  visible: boolean;
  onClick(): void;
  title: string;
}) {
  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      onClick={onClick}
      className={clsx(
        'absolute top-1/2 -translate-y-1/2 rounded-full bg-black/40 p-2 text-white transition-opacity duration-300 hover:bg-black/60',
        side === 'left' ? 'left-3' : 'right-3',
        visible ? 'opacity-100' : 'opacity-0 pointer-events-none',
      )}
    >
      {side === 'left' ? <ChevronLeft size={22} /> : <ChevronRight size={22} />}
    </button>
  );
}
