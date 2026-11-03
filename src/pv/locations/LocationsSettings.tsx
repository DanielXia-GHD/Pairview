// Pairview: Library Locations manager (PRD §6.1.1, docs/IMPLEMENTATION_PLAN.md M1-10).
import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import clsx from 'clsx';
import {
  ChevronDown,
  ChevronUp,
  FolderInput,
  FolderPlus,
  Link,
  Pencil,
  RefreshCw,
  SlidersHorizontal,
  Trash2,
} from 'lucide-react';
import Button from '../../components/ui/Button';
import Dropdown from '../../components/ui/Dropdown';
import Input from '../../components/ui/Input';
import Text from '../../components/ui/Text';
import { TextColors, TextVariants } from '../../types/typography';
import { pvApi, onFolderChanged, type LocationInfo, type LocationStatus, type RelocationSuggestion } from '../api';

const STATUS_REFRESH_MS = 15_000;
/** 0 = the default interval. */
const POLL_OPTIONS = [0, 15, 30, 120, 300];

const statusClass: Record<LocationStatus, string> = {
  online: 'bg-green-500',
  notMounted: 'bg-text-secondary',
  missing: 'bg-yellow-500',
  differentVolume: 'bg-yellow-500',
  unreachable: 'bg-red-500',
};

const stopKeys = (e: React.KeyboardEvent) => e.stopPropagation();

async function pickFolder(title: string): Promise<string | null> {
  const selected = await openDialog({ directory: true, multiple: false, title });
  return typeof selected === 'string' ? selected : null;
}

export default function LocationsSettings() {
  const { t } = useTranslation();
  const [locations, setLocations] = useState<LocationInfo[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setLocations(await pvApi.locations.list());
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(), STATUS_REFRESH_MS);
    const unlisten = onFolderChanged(() => void refresh());
    return () => {
      window.clearInterval(timer);
      void unlisten.then((stop) => stop());
    };
  }, [refresh]);

  /** Runs an action, shows its error, then reloads the list. */
  const run = async (action: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await action();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
      await refresh();
    }
  };

  const add = async () => {
    const path = await pickFolder(t('pv.locations.pickFolder'));
    if (path) await run(() => pvApi.locations.add(path));
  };

  const move = (index: number, by: number) => {
    const ids = locations.map((l) => l.id);
    const [id] = ids.splice(index, 1);
    ids.splice(index + by, 0, id);
    void run(() => pvApi.locations.reorder(ids));
  };

  return (
    <div className="space-y-4">
      <Text variant={TextVariants.small}>{t('pv.locations.intro')}</Text>

      {locations.length === 0 && <Text variant={TextVariants.small}>{t('pv.locations.empty')}</Text>}

      {locations.map((location, index) => (
        <LocationCard
          key={location.id}
          location={location}
          busy={busy}
          canMoveUp={index > 0}
          canMoveDown={index < locations.length - 1}
          onMove={(by) => move(index, by)}
          run={run}
        />
      ))}

      {error && (
        <Text variant={TextVariants.small} color={TextColors.error}>
          {error}
        </Text>
      )}

      <Button onClick={add} disabled={busy}>
        <FolderPlus size={16} />
        {t('pv.locations.add')}
      </Button>
    </div>
  );
}

interface LocationCardProps {
  location: LocationInfo;
  busy: boolean;
  canMoveUp: boolean;
  canMoveDown: boolean;
  onMove(by: number): void;
  run(action: () => Promise<unknown>): Promise<void>;
}

function LocationCard({ location, busy, canMoveUp, canMoveDown, onMove, run }: LocationCardProps) {
  const { t } = useTranslation();
  const [renaming, setRenaming] = useState(false);
  const [name, setName] = useState(location.name);
  const [showSettings, setShowSettings] = useState(false);
  const [excludes, setExcludes] = useState(location.settings.excludes.join(', '));
  const [suggestions, setSuggestions] = useState<RelocationSuggestion[] | null>(null);
  const [confirmRemove, setConfirmRemove] = useState(false);

  useEffect(() => setName(location.name), [location.name]);
  useEffect(() => setExcludes(location.settings.excludes.join(', ')), [location.settings.excludes]);

  const offline = location.status !== 'online';
  const canRelocate =
    location.status === 'missing' || location.status === 'differentVolume' || location.status === 'notMounted';

  const saveName = () => {
    setRenaming(false);
    if (name.trim() && name.trim() !== location.name) void run(() => pvApi.locations.rename(location.id, name.trim()));
  };

  const saveSettings = (patch: Partial<LocationInfo['settings']>) =>
    run(() => pvApi.locations.updateSettings(location.id, { ...location.settings, ...patch }));

  const findRelocation = async () => {
    try {
      setSuggestions(await pvApi.locations.suggestRelocation(location.id));
    } catch {
      setSuggestions([]);
    }
  };

  const relocateTo = (path: string) =>
    run(async () => {
      await pvApi.locations.relocate(location.id, path);
      setSuggestions(null);
    });

  const chooseRelocation = async () => {
    const path = await pickFolder(t('pv.locations.pickNewPath', { name: location.name }));
    if (path) await relocateTo(path);
  };

  return (
    <div className="p-4 rounded-lg bg-bg-primary space-y-3">
      <div className="flex items-start gap-3">
        <span
          className={clsx('mt-2 h-2.5 w-2.5 shrink-0 rounded-full', statusClass[location.status])}
          title={t(`pv.locations.status.${location.status}`)}
        />
        <div className="min-w-0 flex-1">
          {renaming ? (
            <Input
              autoFocus
              value={name}
              onChange={(e) => setName(e.target.value)}
              onBlur={saveName}
              onKeyDown={(e) => {
                stopKeys(e);
                if (e.key === 'Enter') saveName();
                if (e.key === 'Escape') {
                  setName(location.name);
                  setRenaming(false);
                }
              }}
            />
          ) : (
            <Text variant={TextVariants.heading}>{location.name}</Text>
          )}
          <Text variant={TextVariants.small} className="truncate" title={location.path}>
            {location.path}
          </Text>
          <Text variant={TextVariants.small} color={offline ? TextColors.error : TextColors.secondary}>
            {t(`pv.locations.status.${location.status}`)} · {t('pv.locations.photos', { count: location.photos })}
          </Text>
        </div>
        <div className="flex shrink-0 items-center gap-1">
          <IconButton title={t('pv.locations.moveUp')} disabled={busy || !canMoveUp} onClick={() => onMove(-1)}>
            <ChevronUp size={16} />
          </IconButton>
          <IconButton title={t('pv.locations.moveDown')} disabled={busy || !canMoveDown} onClick={() => onMove(1)}>
            <ChevronDown size={16} />
          </IconButton>
          <IconButton title={t('pv.locations.rename')} disabled={busy} onClick={() => setRenaming(true)}>
            <Pencil size={16} />
          </IconButton>
          <IconButton
            title={t('pv.locations.rescan')}
            disabled={busy || offline}
            onClick={() => void run(() => pvApi.locations.rescan(location.id))}
          >
            <RefreshCw size={16} />
          </IconButton>
          <IconButton title={t('pv.locations.settings')} onClick={() => setShowSettings((s) => !s)}>
            <SlidersHorizontal size={16} />
          </IconButton>
        </div>
      </div>

      {(location.connectUrl && location.status === 'notMounted') || canRelocate ? (
        <div className="flex flex-wrap gap-2">
          {location.connectUrl && location.status === 'notMounted' && (
            <SecondaryButton disabled={busy} onClick={() => void run(() => pvApi.locations.connect(location.id))}>
              <Link size={16} />
              {t('pv.locations.connect')}
            </SecondaryButton>
          )}
          {canRelocate && (
            <SecondaryButton disabled={busy} onClick={() => void findRelocation()}>
              <FolderInput size={16} />
              {t('pv.locations.relocate')}
            </SecondaryButton>
          )}
        </div>
      ) : null}

      {suggestions && (
        <div className="space-y-2 rounded-md bg-surface p-3">
          {suggestions.length === 0 ? (
            <Text variant={TextVariants.small}>{t('pv.locations.noSuggestions')}</Text>
          ) : (
            suggestions.map((s) => (
              <div key={s.path} className="flex items-center gap-2">
                <Text variant={TextVariants.small} className="min-w-0 flex-1 truncate" title={s.path}>
                  {s.path}
                  {!s.exact && ` (${t('pv.locations.checkAddress')})`}
                </Text>
                <Button disabled={busy} onClick={() => void relocateTo(s.path)}>
                  {t('pv.locations.useThis')}
                </Button>
              </div>
            ))
          )}
          <SecondaryButton disabled={busy} onClick={() => void chooseRelocation()}>
            {t('pv.locations.chooseFolder')}
          </SecondaryButton>
        </div>
      )}

      {showSettings && (
        <div className="space-y-4 rounded-md bg-surface p-3">
          <div>
            <Text variant={TextVariants.label} className="mb-1 block">
              {t('pv.locations.excludes')}
            </Text>
            <Input
              value={excludes}
              placeholder="exports, 2019/old"
              bgClassName="bg-bg-primary"
              onChange={(e) => setExcludes(e.target.value)}
              onKeyDown={stopKeys}
              onBlur={() => {
                const list = excludes
                  .split(',')
                  .map((x) => x.trim())
                  .filter(Boolean);
                if (list.join(',') !== location.settings.excludes.join(',')) void saveSettings({ excludes: list });
              }}
            />
            <Text variant={TextVariants.small} className="mt-1">
              {t('pv.locations.excludesDesc')}
            </Text>
          </div>
          <div>
            <Text variant={TextVariants.label} className="mb-1 block">
              {t('pv.locations.pollInterval')}
            </Text>
            <Dropdown
              value={location.settings.pollSeconds ?? 0}
              options={POLL_OPTIONS.map((s) => ({
                value: s,
                label: s === 0 ? t('pv.locations.pollDefault') : t('pv.locations.pollSeconds', { count: s }),
              }))}
              onChange={(seconds) => void saveSettings({ pollSeconds: seconds || null })}
              triggerClassName="bg-bg-primary"
            />
            <Text variant={TextVariants.small} className="mt-1">
              {t('pv.locations.pollDesc')}
            </Text>
          </div>
          <div className="flex items-center gap-2">
            {confirmRemove ? (
              <>
                <SecondaryButton
                  danger
                  disabled={busy}
                  onClick={() => void run(() => pvApi.locations.remove(location.id))}
                >
                  <Trash2 size={16} />
                  {t('pv.locations.confirmRemove')}
                </SecondaryButton>
                <SecondaryButton onClick={() => setConfirmRemove(false)}>{t('pv.locations.cancel')}</SecondaryButton>
              </>
            ) : (
              <SecondaryButton disabled={busy} onClick={() => setConfirmRemove(true)}>
                <Trash2 size={16} />
                {t('pv.locations.remove')}
              </SecondaryButton>
            )}
          </div>
          <Text variant={TextVariants.small}>{t('pv.locations.removeDesc')}</Text>
        </div>
      )}
    </div>
  );
}

function IconButton({
  children,
  disabled,
  onClick,
  title,
}: {
  children: React.ReactNode;
  disabled?: boolean;
  onClick(): void;
  title: string;
}) {
  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      disabled={disabled}
      onClick={onClick}
      className="rounded-md p-1.5 text-text-secondary hover:bg-surface hover:text-text-primary disabled:opacity-40 disabled:hover:bg-transparent"
    >
      {children}
    </button>
  );
}

/** Upstream's Button always uses the dark "on accent" text color, unreadable on dark surfaces. */
function SecondaryButton({
  children,
  danger,
  disabled,
  onClick,
}: {
  children: React.ReactNode;
  danger?: boolean;
  disabled?: boolean;
  onClick(): void;
}) {
  return (
    <button
      type="button"
      disabled={disabled}
      onClick={onClick}
      className={clsx(
        'flex items-center justify-center gap-2 rounded-md px-4 py-2 text-md font-semibold',
        'disabled:cursor-not-allowed disabled:opacity-50',
        danger
          ? 'bg-red-600 text-white hover:bg-red-500'
          : 'border border-border-color text-text-primary hover:bg-bg-secondary',
      )}
    >
      {children}
    </button>
  );
}
