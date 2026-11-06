// Pairview: optional film grain + halation per stock (docs/IMPLEMENTATION_PLAN.md M3-11, PRD D17).
// A LUT can't hold grain or anything local, so these set RapidRAW's own sliders: ordinary recipe
// values the owner can then change. Starting points scaled by film speed: faster stocks are grainier,
// slides show less halation, and cinema stocks show more.
import { INITIAL_ADJUSTMENTS, getActiveTools, setToolActive, type Adjustments } from '../../utils/adjustments';

interface FilmEffects {
  grain: number;
  size: number;
  halation: number;
}

const stock = (grain: number, size: number, halation: number): FilmEffects => ({ grain, size, halation });

const FILM_EFFECTS: Record<string, FilmEffects> = {
  'F-C200': stock(13, 25, 12),
  'F-Pro 400H': stock(16, 27, 12),
  'F-Provia 100F': stock(7, 20, 6),
  'F-Velvia 100': stock(7, 20, 6),
  'F-Xtra 400': stock(18, 28, 12),
  'K-Ektachrome 100': stock(7, 20, 6),
  'K-Ektar 100': stock(7, 20, 10),
  'K-Gold 200': stock(14, 26, 12),
  'K-Kodachrome 64': stock(6, 20, 6),
  'K-Portra 160': stock(9, 22, 10),
  'K-Portra 400': stock(15, 26, 12),
  'K-Portra 800': stock(22, 30, 14),
  'K-Portra 800 Push +1': stock(28, 32, 14),
  'K-Portra 800 Push +2': stock(34, 35, 14),
  'K-Ultramax 400': stock(18, 28, 12),
  'K-Verita 200D': stock(13, 25, 12),
  'K-Vision 50D Cinematic': stock(6, 20, 20),
  'K-Vision 200T Cinematic': stock(13, 25, 20),
  'K-Vision 250D Cinematic': stock(14, 25, 20),
  'K-Vision 500T Cinematic': stock(22, 30, 22),
};

export const hasFilmEffects = (lookName: string) => lookName in FILM_EFFECTS;

/** Whether the recipe holds exactly this stock's values. */
export function filmEffectsOn(lookName: string, a: Adjustments): boolean {
  const e = FILM_EFFECTS[lookName];
  return !!e && a.grainAmount === e.grain && a.grainSize === e.size && a.halationAmount === e.halation;
}

export function withFilmEffects(lookName: string, a: Adjustments): Adjustments {
  const e = FILM_EFFECTS[lookName];
  if (!e) return a;
  const tools = setToolActive(setToolActive(getActiveTools(a), 'grain', true), 'creative', true);
  return { ...a, grainAmount: e.grain, grainSize: e.size, halationAmount: e.halation, activeTools: tools };
}

export function withoutFilmEffects(a: Adjustments): Adjustments {
  return {
    ...a,
    grainAmount: INITIAL_ADJUSTMENTS.grainAmount,
    grainSize: INITIAL_ADJUSTMENTS.grainSize,
    halationAmount: INITIAL_ADJUSTMENTS.halationAmount,
  };
}
