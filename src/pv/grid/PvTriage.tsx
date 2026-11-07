// Pairview: the AI triage button and summary in the library header (docs/M4-triage.md). Nothing
// runs until the owner presses it, and the results are suggestions: no file, rating or flag changes.
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'react-toastify';
import { Sparkles } from 'lucide-react';
import { pvApi, type ShootTriage } from '../api';
import { offlineLocationFor, usePvLocations } from '../offline';
import { usePvPhotoStore } from '../store';
import { stillSuggestedReject, usePvTriageStore } from './triage';

interface Progress {
  folderId: number;
  done: number;
  total: number;
}

export default function PvTriage({ path }: { path: string | null }) {
  const { t } = useTranslation();
  const [folderId, setFolderId] = useState<number | null>(null);
  const [results, setResults] = useState<ShootTriage | null>(null);
  const [running, setRunning] = useState(false);
  const [progress, setProgress] = useState<Progress | null>(null);
  const setStore = usePvTriageStore((s) => s.setResults);
  const byPhoto = usePvTriageStore((s) => s.byPhoto);
  const photos = usePvPhotoStore((s) => s.byPath);
  usePvLocations((s) => s.locations);
  const offline = offlineLocationFor(path);

  useEffect(() => {
    let cancelled = false;
    setFolderId(null);
    setResults(null);
    setStore(null);
    setProgress(null);
    if (!path) return;
    pvApi
      .shootIntent(path)
      .then(async (shoot) => {
        if (cancelled || !shoot) return;
        setFolderId(shoot.folderId);
        const found = await pvApi.triageResults(shoot.folderId);
        if (cancelled) return;
        setResults(found);
        setStore(found);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [path, setStore]);

  useEffect(() => {
    const unlisten = listen<Progress>('pv-triage-progress', (e) => setProgress(e.payload));
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, []);

  if (folderId === null || offline) return null;

  const run = async () => {
    setRunning(true);
    setProgress(null);
    try {
      const found = await pvApi.triageRun(folderId);
      setResults(found);
      setStore(found);
    } catch (e) {
      toast.error(String(e));
    } finally {
      setRunning(false);
    }
  };

  const shown = Object.values(photos);
  const rejects = shown.filter((p) => stillSuggestedReject(p, byPhoto[p.id])).length;
  const bursts = new Set(shown.map((p) => byPhoto[p.id]?.burst).filter((b) => b != null)).size;
  const measured = results?.measured ?? 0;
  const unread = (results?.photos ?? 0) - measured;

  let label = t('pv.triage.run');
  if (running) {
    label =
      progress && progress.folderId === folderId && progress.total > 0
        ? t('pv.triage.progress', { done: progress.done, total: progress.total })
        : t('pv.triage.running');
  } else if (measured > 0) {
    label = t('pv.triage.summary', { rejects, bursts });
  }

  return (
    <button
      type="button"
      disabled={running}
      onClick={() => void run()}
      className="flex shrink-0 items-center gap-1 rounded-full bg-surface px-2 py-0.5 text-xs text-text-secondary transition-colors hover:text-text-primary disabled:opacity-70"
      data-tooltip={unread > 0 && measured > 0 ? t('pv.triage.unread', { count: unread }) : t('pv.triage.hint')}
    >
      <Sparkles size={12} />
      {label}
    </button>
  );
}
