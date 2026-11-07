// Pairview: RAW Photos start from "My Sony" (docs/IMPLEMENTATION_PLAN.md M3-07, PRD §6.4).
// A RAW with no recipe opens looking like the camera's file, not RapidRAW's dark neutral render.
// The look is an ordinary LUT in the recipe: visible in the Effects panel, removable, with an
// intensity. Only a Photo's default changes; a saved recipe is never touched.
import { invoke } from '@tauri-apps/api/core';
import { getActiveTools, setToolActive, type Adjustments } from '../../utils/adjustments';
import { pvPhotoForPath } from '../store';
import { isSameLook } from '../looks/sameLook';

interface BaseLook {
  path: string;
  name: string;
  size: number;
}

let look: BaseLook | null = null;

/** Loads the look once at startup, so opening a Photo needn't wait for it. */
export function loadBaseLook() {
  void invoke<BaseLook | null>('pv_base_look')
    .then((found) => {
      look = found;
    })
    .catch(() => {
      look = null;
    });
}

loadBaseLook();

const hasRaw = (path: string) => !!pvPhotoForPath(path)?.rawExt;

/** Default adjustments for `path`: with the look when it's a Pairview RAW Photo. */
export function withBaseLook<T extends Adjustments>(path: string | null | undefined, adjustments: T): T {
  if (!look || !path || !hasRaw(path)) return adjustments;
  return {
    ...adjustments,
    lutPath: look.path,
    lutName: look.name,
    lutSize: look.size,
    lutIntensity: 100,
    lutIsSceneReferred: false,
    activeTools: setToolActive(getActiveTools(adjustments), 'lut', true),
  };
}

/** "Before" in before/after: the camera look stays on, only the owner's edits go. */
export function keepBaseLook<T extends Adjustments>(original: T, current: Adjustments): T {
  if (!look || !isSameLook(current.lutPath, look.path)) return original;
  return {
    ...original,
    lutPath: current.lutPath,
    lutName: current.lutName,
    lutSize: current.lutSize,
    lutIntensity: current.lutIntensity,
    lutIsSceneReferred: current.lutIsSceneReferred,
    activeTools: setToolActive(getActiveTools(original), 'lut', true),
  };
}
