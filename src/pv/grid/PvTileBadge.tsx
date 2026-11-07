// Pairview: Photo badge on a library tile (PRD §5.3): RAW+HIF, RAW+JPG, HIF, RAW, ⚠ RAW, …
import { useTranslation } from 'react-i18next';
import { AlertTriangle, Layers, Lock, ThumbsDown } from 'lucide-react';
import { basePath, usePvPhotoStore } from '../store';
import type { PhotoItem } from '../api';
import { stillSuggestedReject, usePvTriageStore } from './triage';

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
  const triage = usePvTriageStore((s) => (photo ? s.byPhoto[photo.id] : undefined));
  if (!photo) return null;
  const suggestReject = stillSuggestedReject(photo, triage);

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
    <>
      <div
        className="absolute top-1.5 left-1.5 z-10 flex items-center gap-1 rounded-full bg-black/45 px-1.5 h-5 text-[10px] font-semibold tracking-wide text-white shadow-md pointer-events-auto"
        data-tooltip={tooltip}
      >
        {photo.guardMismatch && <AlertTriangle size={11} className="text-yellow-300" />}
        <span className={photo.state === 'orphan_raw' ? 'text-yellow-300' : undefined}>{badgeLabel(photo)}</span>
        {photo.keepRaw === 'keep' && <Lock size={10} />}
      </div>
      {triage && (
        // Dimmed, and apart from the owner's own stars: this is the AI's opinion, not a rating.
        <div
          className="absolute bottom-1.5 left-1.5 z-10 flex items-center gap-1 rounded-full bg-black/35 px-1.5 py-0.5 text-[10px] text-white/70"
          data-tooltip={[
            t('pv.triage.tile.stars', { count: triage.aiStars }),
            triage.burstSize > 1
              ? t(triage.bestOfBurst ? 'pv.triage.tile.best' : 'pv.triage.tile.burst', { count: triage.burstSize })
              : null,
            suggestReject && triage.reject ? t(`pv.triage.reject.${triage.reject}`) : null,
          ]
            .filter(Boolean)
            .join(' · ')}
        >
          <span>{`AI ${'★'.repeat(triage.aiStars) || '–'}`}</span>
          {triage.burstSize > 1 && <Layers size={10} className={triage.bestOfBurst ? 'text-white' : undefined} />}
          {suggestReject && <ThumbsDown size={10} className="text-amber-300/80" />}
        </div>
      )}
    </>
  );
}
