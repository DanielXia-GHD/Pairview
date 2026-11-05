// Pairview: offline locations across the UI (docs/IMPLEMENTATION_PLAN.md M2-10).
// The backend refuses every change on an offline location; this lets the UI say so up front
// (a read-only badge, disabled controls, one clear message) instead of failing with errors.
import { create } from 'zustand';
import { toast } from 'react-toastify';
import i18n from 'i18next';
import { pvApi, type LocationInfo } from './api';

const REFRESH_MS = 30_000;

interface LocationsState {
  locations: LocationInfo[];
}

export const usePvLocations = create<LocationsState>(() => ({ locations: [] }));

let timer: number | null = null;

export async function refreshLocations() {
  try {
    usePvLocations.setState({ locations: await pvApi.locations.list() });
  } catch {
    // Backend not ready: keep the last known statuses.
  }
}

/** Starts the periodic status refresh (idempotent). */
export function watchLocations() {
  if (timer !== null) return;
  void refreshLocations();
  timer = window.setInterval(() => void refreshLocations(), REFRESH_MS);
  window.addEventListener('focus', () => void refreshLocations());
}

const inside = (path: string, root: string) =>
  path === root || path.startsWith(root.endsWith('/') ? root : `${root}/`) || path.startsWith(`${root}\\`);

/** The location holding `path`, if it is known and not online. */
export function offlineLocationFor(path: string | null | undefined): LocationInfo | null {
  if (!path) return null;
  const location = usePvLocations
    .getState()
    .locations.filter((l) => inside(path.split('?vc=')[0], l.path))
    .sort((a, b) => b.path.length - a.path.length)[0];
  return location && location.status !== 'online' ? location : null;
}

/** True (after telling the owner) when any of `paths` is on an offline location. */
export function refuseIfOffline(paths: Array<string | null | undefined>): boolean {
  const offline = paths.map(offlineLocationFor).find(Boolean);
  if (!offline) return false;
  toast.warn(i18n.t('pv.offline.refused', { name: offline.name }));
  return true;
}
