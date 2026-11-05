// Pairview: shoot intent chip in the library header (docs/IMPLEMENTATION_PLAN.md M2-08, PRD §6.5).
// Casual · Normal · Important for the open shoot; it moves the Keep RAW / HIF is enough line for
// every Photo in it. Hidden for anything that isn't a shoot (date views, folders of folders).
// On an offline location it shows a read-only badge instead of controls (M2-10).
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import clsx from 'clsx';
import { toast } from 'react-toastify';
import { WifiOff } from 'lucide-react';
import { pvApi, type ShootIntent, type ShootIntentValue } from '../api';
import { offlineLocationFor, usePvLocations } from '../offline';

const INTENTS: ShootIntentValue[] = ['casual', 'normal', 'important'];

export default function PvShootIntent({ path }: { path: string | null }) {
  const { t } = useTranslation();
  const [shoot, setShoot] = useState<ShootIntent | null>(null);
  // Re-render when location statuses change.
  usePvLocations((s) => s.locations);
  const offline = offlineLocationFor(path);

  useEffect(() => {
    let cancelled = false;
    setShoot(null);
    if (!path) return;
    pvApi
      .shootIntent(path)
      .then((s) => !cancelled && setShoot(s))
      .catch(() => !cancelled && setShoot(null));
    return () => {
      cancelled = true;
    };
  }, [path]);

  if (offline) {
    return (
      <span
        className="flex shrink-0 items-center gap-1 rounded-full bg-amber-500/15 px-2 py-0.5 text-xs text-amber-300"
        data-tooltip={t('pv.offline.hint')}
      >
        <WifiOff size={12} />
        {t('pv.offline.badge', { name: offline.name })}
      </span>
    );
  }
  if (!shoot) return null;

  const choose = async (intent: ShootIntentValue) => {
    if (intent === shoot.intent) return;
    const previous = shoot;
    setShoot({ ...shoot, intent });
    try {
      await pvApi.setShootIntent(shoot.folderId, intent);
    } catch (e) {
      setShoot(previous);
      toast.error(String(e));
    }
  };

  return (
    <div
      className="flex shrink-0 items-center rounded-full bg-surface p-0.5 text-xs"
      role="radiogroup"
      aria-label={t('pv.intent.label')}
      data-tooltip={t('pv.intent.hint')}
    >
      {INTENTS.map((intent) => (
        <button
          key={intent}
          type="button"
          role="radio"
          aria-checked={shoot.intent === intent}
          onClick={() => void choose(intent)}
          className={clsx(
            'rounded-full px-2 py-0.5 transition-colors',
            shoot.intent === intent
              ? intent === 'important'
                ? 'bg-emerald-600 text-white'
                : intent === 'casual'
                  ? 'bg-bg-primary text-text-primary'
                  : 'bg-accent text-button-text'
              : 'text-text-secondary hover:text-text-primary',
          )}
        >
          {t(`pv.intent.${intent}`)}
        </button>
      ))}
    </div>
  );
}
