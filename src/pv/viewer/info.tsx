// Pairview: viewer info overlay, toggled with I (docs/IMPLEMENTATION_PLAN.md M1-16).
// Shows the Photo's histogram (RapidRAW's own calculation, drawn the same way as its editor's),
// capture EXIF, and every member file with its size: the RAW and its display copy side by side.
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { formatBytes } from '../../utils/formatBytes';
import { pvApi, type Histogram, type MemberFile, type PhotoDetail, type PhotoItem } from '../api';

const STORAGE_KEY = 'pv.viewer.info';
/** Waits this long before loading, so holding → doesn't queue a request per Photo. */
const LOAD_DELAY_MS = 120;

export function loadInfoOpen(): boolean {
  try {
    return localStorage.getItem(STORAGE_KEY) === '1';
  } catch {
    return false;
  }
}

export function saveInfoOpen(open: boolean) {
  try {
    localStorage.setItem(STORAGE_KEY, open ? '1' : '0');
  } catch {
    // Private storage: the panel just starts closed next time.
  }
}

// ---- Formatting ----

export function formatExposure(seconds: number): string {
  if (seconds >= 1) return `${Number(seconds.toFixed(1))} s`;
  return `1/${Math.round(1 / seconds)} s`;
}

export function formatAperture(f: number): string {
  return `f/${Number(f.toFixed(1))}`;
}

export function formatDimensions(width: number, height: number): string {
  const mp = (width * height) / 1e6;
  return `${width} × ${height} · ${mp >= 10 ? Math.round(mp) : mp.toFixed(1)} MP`;
}

export function formatCoordinate(value: number, positive: string, negative: string): string {
  return `${Math.abs(value).toFixed(5)}° ${value >= 0 ? positive : negative}`;
}

const fileName = (path: string) => path.split(/[\\/]/).pop() ?? path;

// ---- Panel ----

export default function InfoPanel({ photo }: { photo: PhotoItem }) {
  const { t } = useTranslation();
  const [detail, setDetail] = useState<PhotoDetail | null>(null);
  const [histogram, setHistogram] = useState<Histogram | null>(null);
  const [histogramFailed, setHistogramFailed] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setHistogramFailed(false);
    const timer = window.setTimeout(() => {
      pvApi
        .photoDetail(photo.id)
        .then((d) => !cancelled && setDetail(d))
        .catch(() => !cancelled && setDetail(null));
      pvApi
        .histogram(photo.id)
        .then((h) => !cancelled && setHistogram(h))
        .catch(() => {
          if (cancelled) return;
          setHistogram(null);
          setHistogramFailed(true);
        });
    }, LOAD_DELAY_MS);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [photo.id]);

  // Until this Photo's detail arrives, the previous one stays up rather than flashing empty.
  const exif = detail?.exif;
  const files = (detail?.files ?? []).filter((f) => f.role !== 'sidecar');
  const sidecars = (detail?.files ?? []).filter((f) => f.role === 'sidecar');
  const total = files.reduce((sum, f) => sum + f.size, 0);

  const exposure = [
    exif?.exposureS ? formatExposure(exif.exposureS) : null,
    exif?.fNumber ? formatAperture(exif.fNumber) : null,
    exif?.iso ? `ISO ${exif.iso}` : null,
    exif?.focalMm ? `${Number(exif.focalMm.toFixed(1))} mm` : null,
  ].filter(Boolean);
  const camera = [exif?.make, exif?.model].filter(Boolean).join(' ') || photo.camera;
  const captured = exif?.dateOriginal
    ? `${exif.dateOriginal.replace('T', ' ')}${exif.offsetOriginal ? ` (UTC${exif.offsetOriginal})` : ''}`
    : photo.capturedAt?.replace('T', ' ').slice(0, 19);

  return (
    <aside
      className="absolute right-3 top-16 bottom-16 flex w-72 flex-col gap-4 overflow-y-auto rounded-lg bg-black/70 p-4 text-xs text-white/90 backdrop-blur-md"
      aria-label={t('pv.info.title')}
      onWheel={(e) => e.stopPropagation()}
      onMouseDown={(e) => e.stopPropagation()}
    >
      <div className="h-24 shrink-0 rounded bg-black/40 p-1">
        {histogram ? (
          <HistogramChart data={histogram} />
        ) : (
          <div className="flex h-full items-center justify-center text-white/40">
            {histogramFailed ? t('pv.info.noHistogram') : '…'}
          </div>
        )}
      </div>

      <Section title={t('pv.info.capture')}>
        {captured && (
          <Row label={t('pv.info.date')}>
            {captured}
            {photo.dateUncertain && <span className="ml-1 text-amber-300">{t('pv.info.dateUncertain')}</span>}
          </Row>
        )}
        {camera && <Row label={t('pv.info.camera')}>{camera}</Row>}
        {(exif?.lens ?? photo.lens) && <Row label={t('pv.info.lens')}>{exif?.lens ?? photo.lens}</Row>}
        {exposure.length > 0 && <Row label={t('pv.info.exposure')}>{exposure.join(' · ')}</Row>}
        {exif?.width && exif?.height && (
          <Row label={t('pv.info.dimensions')}>{formatDimensions(exif.width, exif.height)}</Row>
        )}
        {exif?.gpsLat != null && exif?.gpsLon != null && (
          <Row label={t('pv.info.location')}>
            {formatCoordinate(exif.gpsLat, 'N', 'S')}, {formatCoordinate(exif.gpsLon, 'E', 'W')}
          </Row>
        )}
        {!exif && detail && <div className="text-white/50">{t('pv.info.noExif')}</div>}
      </Section>

      <Section title={t('pv.info.files')}>
        {files.map((f) => (
          <FileRow key={f.path} file={f} />
        ))}
        {files.length > 1 && (
          <Row label={t('pv.info.total')}>
            <span className="tabular-nums">{formatBytes(total, t, 1)}</span>
          </Row>
        )}
        {sidecars.length > 0 && (
          <div className="truncate text-white/50" title={sidecars.map((f) => f.path).join('\n')}>
            {t('pv.info.sidecars', { names: sidecars.map((f) => fileName(f.path)).join(', ') })}
          </div>
        )}
        {photo.guardMismatch && <div className="text-amber-300">{t('pv.info.guardMismatch')}</div>}
      </Section>
    </aside>
  );
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="flex flex-col gap-1.5">
      <h3 className="text-[10px] font-semibold uppercase tracking-wider text-white/50">{title}</h3>
      {children}
    </section>
  );
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex gap-2">
      <span className="w-20 shrink-0 text-white/50">{label}</span>
      <span className="min-w-0 flex-1 break-words">{children}</span>
    </div>
  );
}

function FileRow({ file }: { file: MemberFile }) {
  const { t } = useTranslation();
  return (
    <div className="flex items-center gap-2" title={file.path}>
      <span className="w-20 shrink-0">
        <span className="rounded bg-white/15 px-1.5 py-0.5 text-[10px] font-semibold uppercase">
          {t(`pv.info.role.${file.role as 'raw' | 'display' | 'extra'}`)}
        </span>
      </span>
      <span className="min-w-0 flex-1 truncate">{fileName(file.path)}</span>
      <span className="shrink-0 tabular-nums text-white/70">{formatBytes(file.size, t, 1)}</span>
    </div>
  );
}

/** Drawn like RapidRAW's editor histogram (src/components/panel/editor/Waveform.tsx). */
function HistogramChart({ data }: { data: Histogram }) {
  const max = Math.max(...data.red, ...data.green, ...data.blue, 1e-6);
  const points = (values: number[]) => values.map((v, i) => `${i},${255 - (v / max) * 255}`).join(' L');
  const channels = [
    { key: 'red', color: '#FF6B6B', values: data.red },
    { key: 'green', color: '#6BCB77', values: data.green },
    { key: 'blue', color: '#4D96FF', values: data.blue },
  ];
  return (
    <svg viewBox="0 0 255 255" className="h-full w-full" preserveAspectRatio="none" aria-hidden="true">
      {channels.map((ch) => (
        <g key={ch.key} style={{ mixBlendMode: 'lighten' }}>
          <path d={`M0,255 L${points(ch.values)} L255,255 Z`} fill={ch.color} fillOpacity={0.4} />
          <path
            d={`M${points(ch.values)}`}
            fill="none"
            stroke={ch.color}
            strokeWidth={1.5}
            vectorEffect="non-scaling-stroke"
            strokeLinejoin="round"
          />
        </g>
      ))}
    </svg>
  );
}
