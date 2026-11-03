// Pairview: the date view (PRD §6.1, docs/IMPLEMENTATION_PLAN.md M1-13). Dates are browsed through
// virtual library paths, `Dates: 2026`, `Dates: 2026-10`, `Dates: 2026-10-31` (like upstream's
// `Album: …`), which the backend lists by capture date.
import i18n from 'i18next';
import { pvPhotoForPath } from './store';

export const PV_DATES_PREFIX = 'Dates: ';

export const isPvDatesPath = (path: string | null | undefined): boolean => !!path?.startsWith(PV_DATES_PREFIX);

export const datesPath = (spec: string) => `${PV_DATES_PREFIX}${spec}`;

const localDate = (day: string) => {
  const [y, m, d] = day.split('-').map(Number);
  return new Date(y, (m || 1) - 1, d || 1);
};

export const formatDay = (day: string) =>
  localDate(day).toLocaleDateString(i18n.language, { weekday: 'long', day: 'numeric', month: 'long', year: 'numeric' });

export const formatMonth = (month: string) =>
  localDate(`${month}-01`).toLocaleDateString(i18n.language, { month: 'long' });

export const formatWeekday = (day: string) => localDate(day).toLocaleDateString(i18n.language, { weekday: 'short' });

/** Day groups for the grid in date view, oldest day first; images keep their sorted order. */
export function groupImagesByDay<T extends { path: string }>(images: T[]): Array<{ path: string; images: T[] }> {
  const byDay = new Map<string, T[]>();
  for (const image of images) {
    const day = pvPhotoForPath(image.path)?.capturedAt?.slice(0, 10) ?? '';
    const list = byDay.get(day);
    if (list) list.push(image);
    else byDay.set(day, [image]);
  }
  return [...byDay.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([day, dayImages]) => ({ path: day ? formatDay(day) : i18n.t('pv.dates.undated'), images: dayImages }));
}
