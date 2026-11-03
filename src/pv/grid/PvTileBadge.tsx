// Pairview: Photo badge on a library tile (PRD §5.3): RAW+HIF, RAW+JPG, HIF, RAW, ⚠ RAW, …
import { useTranslation } from 'react-i18next';
import { AlertTriangle, Lock } from 'lucide-react';
import { basePath, usePvPhotoStore } from '../store';
import type { PhotoItem } from '../api';

export function badgeLabel(photo: PhotoItem): string {
  const display = (photo.displayExt ?? '').toUpperCase().replace('JPEG', 'JPG');
  switch (photo.state) {
    case 'raw_display':
      return `RAW+${display}`;
    case 'edited':
      return `✎ RAW+${display}`;
    case 'display_only':
      return display;
    case 'raw_only':
      return 'RAW';
    case 'orphan_raw':
      return '⚠ RAW';
  }
}

export default function PvTileBadge({ path }: { path: string }) {
  const { t } = useTranslation();
  const photo = usePvPhotoStore((s) => s.byPath[basePath(path)]);
  if (!photo) return null;

  const tooltip = [
    t(`pv.grid.state.${photo.state}`),
    photo.guardMismatch ? t('pv.grid.mismatch') : null,
    photo.keepRaw === 'keep' ? t('pv.grid.keepRaw') : null,
    photo.keepRaw === 'drop' ? t('pv.grid.dropRaw') : null,
    photo.dateUncertain ? t('pv.grid.dateUncertain') : null,
  ]
    .filter(Boolean)
    .join(' · ');

  return (
    <div
      className="absolute top-1.5 left-1.5 z-10 flex items-center gap-1 rounded-full bg-black/45 px-1.5 h-5 text-[10px] font-semibold tracking-wide text-white shadow-md pointer-events-auto"
      data-tooltip={tooltip}
    >
      {photo.guardMismatch && <AlertTriangle size={11} className="text-yellow-300" />}
      <span className={photo.state === 'orphan_raw' ? 'text-yellow-300' : undefined}>{badgeLabel(photo)}</span>
      {photo.keepRaw === 'keep' && <Lock size={10} />}
    </div>
  );
}
