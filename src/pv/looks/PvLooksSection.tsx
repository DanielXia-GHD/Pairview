// Pairview: Looks panel (docs/IMPLEMENTATION_PLAN.md M3-07, PRD §6.4, D18). Live thumbnails grouped
// as My Camera · Film · Camera styles, with favourites and intensity. A look is one LUT in the
// recipe, so choosing one replaces the last. Upstream's LUT control in Effects stays for the owner's
// own LUT files. Hidden for files outside a Pairview location.
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useTranslation } from 'react-i18next';
import clsx from 'clsx';
import { Ban, ImageOff, Star } from 'lucide-react';
import CollapsibleSection from '../../components/ui/CollapsibleSection';
import Slider from '../../components/ui/Slider';
import { useEditorStore } from '../../store/useEditorStore';
import type { Adjustments } from '../../utils/adjustments';
import { pvPhotoForPath, usePvPhotoStore } from '../store';

type LookGroup = 'myCamera' | 'film' | 'cameraStyle';

interface Look {
  name: string;
  path: string;
  isBuiltIn: boolean;
  group: LookGroup;
}

interface LookPreview {
  path: string;
  thumb: string | null;
}

interface Props {
  setAdjustments(update: (prev: Adjustments) => Adjustments, label?: string): void;
  onSelect(path: string, isBuiltIn: boolean): void;
  onHover(path: string | null, isBuiltIn?: boolean): void;
  onDragStateChange(isDragging: boolean): void;
}

const GROUPS: LookGroup[] = ['myCamera', 'film', 'cameraStyle'];
const PREVIEW_PX = 112;
/** Lets the photo finish appearing before thumbnails take the GPU. */
const PREVIEW_DELAY_MS = 250;
const OPEN_KEY = 'pv.looks.open';
const FAVOURITES_KEY = 'pv.looks.favourites';

function loadOpen(): boolean {
  try {
    return localStorage.getItem(OPEN_KEY) !== '0';
  } catch {
    return true;
  }
}

function loadFavourites(): string[] {
  try {
    const value = JSON.parse(localStorage.getItem(FAVOURITES_KEY) ?? '[]');
    return Array.isArray(value) ? value : [];
  } catch {
    return [];
  }
}

function save(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Private storage: this just isn't remembered.
  }
}

export default function PvLooksSection({ setAdjustments, onSelect, onHover, onDragStateChange }: Props) {
  const { t } = useTranslation();
  const imagePath = useEditorStore((s) => s.selectedImage?.path ?? null);
  const isReady = useEditorStore((s) => s.selectedImage?.isReady ?? false);
  const lutPath = useEditorStore((s) => s.adjustments.lutPath ?? null);
  const lutIntensity = useEditorStore((s) => s.adjustments.lutIntensity ?? 100);
  usePvPhotoStore((s) => s.byPath);
  const photo = imagePath ? pvPhotoForPath(imagePath) : undefined;

  const [looks, setLooks] = useState<Look[]>([]);
  const [isOpen, setIsOpen] = useState(loadOpen);
  const [favourites, setFavourites] = useState(loadFavourites);
  const [previews, setPreviews] = useState<Record<string, string | null>>({});
  const previewCache = useRef(new Map<string, Record<string, string | null>>());

  useEffect(() => {
    invoke<Look[]>('pv_looks')
      .then(setLooks)
      .catch(() => setLooks([]));
  }, []);

  // My Camera looks are fitted on RAW renders; they don't apply to a Photo without a RAW.
  const shown = useMemo(() => looks.filter((l) => l.group !== 'myCamera' || !!photo?.rawExt), [looks, photo?.rawExt]);

  useEffect(() => {
    if (!isOpen || !imagePath || !isReady || !photo || shown.length === 0) return;
    const cacheKey = `${imagePath}|${shown.map((l) => l.path).join(',')}`;
    const cached = previewCache.current.get(cacheKey);
    if (cached) {
      setPreviews(cached);
      return;
    }
    let active = true;
    setPreviews({});
    const timer = window.setTimeout(() => {
      invoke<LookPreview[]>('generate_lut_previews', {
        luts: shown.map((l) => ({ path: l.path, isBuiltIn: l.isBuiltIn })),
        size: PREVIEW_PX,
      })
        .then((results) => {
          const map = Object.fromEntries(results.map((r) => [r.path, r.thumb]));
          previewCache.current.set(cacheKey, map);
          if (active) setPreviews(map);
        })
        .catch((e) => console.warn('Look previews failed:', e));
    }, PREVIEW_DELAY_MS);
    return () => {
      active = false;
      window.clearTimeout(timer);
    };
    // `photo` only gates this; its fields changing mustn't re-render every thumbnail.
  }, [isOpen, imagePath, isReady, shown, !!photo]);

  // Leaving the panel mid-hover must not leave a preview look on the image.
  useEffect(() => () => onHover(null), [onHover]);

  const toggleOpen = () => {
    setIsOpen((open) => {
      save(OPEN_KEY, open ? '0' : '1');
      return !open;
    });
  };

  const toggleFavourite = (name: string) => {
    setFavourites((list) => {
      const next = list.includes(name) ? list.filter((n) => n !== name) : [...list, name];
      save(FAVOURITES_KEY, JSON.stringify(next));
      return next;
    });
  };

  const clearLook = useCallback(() => {
    onHover(null);
    setAdjustments(
      (prev) => ({
        ...prev,
        lutPath: null,
        lutName: null,
        lutData: null,
        lutSize: 0,
        lutIntensity: 100,
        lutIsSceneReferred: false,
      }),
      t('pv.looks.none'),
    );
  }, [onHover, setAdjustments, t]);

  if (!photo || !imagePath || shown.length === 0) return null;

  const renderLook = (look: Look, keyPrefix: string) => {
    const thumb = previews[look.path];
    const isFavourite = favourites.includes(look.name);
    return (
      <div
        key={keyPrefix + look.path}
        role="button"
        tabIndex={0}
        aria-pressed={look.path === lutPath}
        onMouseEnter={() => onHover(look.path, look.isBuiltIn)}
        onMouseLeave={() => onHover(null)}
        onClick={() => {
          onHover(null);
          if (look.path !== lutPath) onSelect(look.path, look.isBuiltIn);
        }}
        onKeyDown={(e) => {
          if ((e.key === 'Enter' || e.key === ' ') && look.path !== lutPath) {
            e.preventDefault();
            e.stopPropagation();
            onSelect(look.path, look.isBuiltIn);
          }
        }}
        className={clsx(
          'group/look relative aspect-square cursor-pointer overflow-hidden rounded-md border-2 bg-bg-tertiary transition-colors',
          look.path === lutPath ? 'border-accent' : 'border-transparent hover:border-surface',
        )}
        data-tooltip={look.name}
      >
        {thumb === undefined ? (
          <div className="h-full w-full animate-pulse bg-surface" />
        ) : thumb ? (
          <img src={thumb} alt="" className="h-full w-full object-cover" draggable={false} />
        ) : (
          <div className="flex h-full w-full items-center justify-center text-text-secondary">
            <ImageOff size={18} />
          </div>
        )}
        <button
          type="button"
          className={clsx(
            'absolute right-0.5 top-0.5 rounded-full bg-black/50 p-0.5 text-white transition-opacity',
            isFavourite ? 'opacity-100' : 'opacity-0 group-hover/look:opacity-100 focus:opacity-100',
          )}
          aria-pressed={isFavourite}
          aria-label={t(isFavourite ? 'pv.looks.unfavourite' : 'pv.looks.favourite')}
          onClick={(e) => {
            e.stopPropagation();
            toggleFavourite(look.name);
          }}
        >
          <Star size={12} fill={isFavourite ? 'currentColor' : 'none'} />
        </button>
        <span className="absolute inset-x-0 bottom-0 truncate bg-black/60 px-1 py-0.5 text-left text-[10px] text-white">
          {look.name}
        </span>
      </div>
    );
  };

  const favouriteLooks = shown.filter((l) => favourites.includes(l.name));

  return (
    <div className="shrink-0 group">
      <CollapsibleSection
        canToggleVisibility={false}
        isContentVisible
        isOpen={isOpen}
        onToggle={toggleOpen}
        title={t('pv.looks.title')}
      >
        <div className="space-y-3 pb-1">
          {lutPath && (
            <Slider
              label={t('pv.looks.intensity')}
              min={0}
              max={100}
              step={1}
              value={lutIntensity}
              defaultValue={100}
              onChange={(e) => {
                const value = Number(e.target.value);
                setAdjustments((prev) => ({ ...prev, lutIntensity: value }));
              }}
              onDragStateChange={onDragStateChange}
              fillOrigin="min"
            />
          )}
          {favouriteLooks.length > 0 && (
            <div>
              <span className="mb-2 block select-none text-sm font-medium text-text-secondary">
                {t('pv.looks.groups.favourites')}
              </span>
              <div className="grid grid-cols-3 gap-2">{favouriteLooks.map((l) => renderLook(l, 'favourite:'))}</div>
            </div>
          )}
          {GROUPS.map((group, index) => {
            const inGroup = shown.filter((l) => l.group === group);
            if (inGroup.length === 0 && index > 0) return null;
            return (
              <div key={group}>
                {inGroup.length > 0 && (
                  <span className="mb-2 block select-none text-sm font-medium text-text-secondary">
                    {t(`pv.looks.groups.${group}`)}
                  </span>
                )}
                <div className="grid grid-cols-3 gap-2">
                  {inGroup.map((l) => renderLook(l, ''))}
                  {group === 'myCamera' && (
                    <button
                      type="button"
                      onClick={clearLook}
                      className={clsx(
                        'relative flex aspect-square flex-col items-center justify-center gap-1 rounded-md border-2 bg-bg-tertiary text-text-secondary transition-colors',
                        !lutPath ? 'border-accent' : 'border-transparent hover:border-surface',
                      )}
                      data-tooltip={t(photo.rawExt ? 'pv.looks.noneHintRaw' : 'pv.looks.noneHintDisplay')}
                    >
                      <Ban size={18} />
                      <span className="text-[10px]">{t('pv.looks.none')}</span>
                    </button>
                  )}
                </div>
              </div>
            );
          })}
        </div>
      </CollapsibleSection>
    </div>
  );
}
