// Pairview: shoot intent chip in the library header (docs/IMPLEMENTATION_PLAN.md M2-08, PRD §6.5).
// Casual · Normal · Important for the open shoot; it moves the Keep RAW / HIF is enough line for
// every Photo in it. Hidden for anything that isn't a shoot (date views, folders of folders).
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import clsx from 'clsx';
import { toast } from 'react-toastify';
import { pvApi, type ShootIntent, type ShootIntentValue } from '../api';

const INTENTS: ShootIntentValue[] = ['casual', 'normal', 'important'];

export default function PvShootIntent({ path }: { path: string | null }) {
  const { t } = useTranslation();
  const [shoot, setShoot] = useState<ShootIntent | null>(null);

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
