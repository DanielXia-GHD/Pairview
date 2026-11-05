// Pairview: typed bindings for the backend commands in src-tauri/src/pv/commands.rs.
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { ImageFile } from '../components/ui/AppProperties';
import { usePvPhotoStore } from './store';

// ---- Types (mirror the Rust structs, camelCase) ----

export type LocationStatus = 'online' | 'notMounted' | 'missing' | 'differentVolume' | 'unreachable';

export interface LocationInfo {
  id: number;
  name: string;
  path: string;
  status: LocationStatus;
  /** `smb://…` to offer Connect when the share isn't mounted. */
  connectUrl: string | null;
  photos: number;
  sortOrder: number;
  lastScanAt: number | null;
  settings: LocationSettings;
}

export interface LocationSettings {
  /** Folder names, or paths relative to the location, to skip. */
  excludes: string[];
  /** Watcher poll interval; null = default (60 s). */
  pollSeconds: number | null;
}

export interface RelocationSuggestion {
  path: string;
  /** false: same share name on another server address; ask the owner to check. */
  exact: boolean;
}

export interface FolderNode {
  id: number;
  parentId: number | null;
  name: string;
  relPath: string;
  path: string;
  /** `arw/`, `hif/`, …: part of the parent shoot, normally hidden. */
  formatSubdir: boolean;
  dateHint: string | null;
  photos: number;
  photosRecursive: number;
}

export type PhotoState = 'raw_display' | 'edited' | 'display_only' | 'raw_only' | 'orphan_raw';
export type Flag = 'none' | 'pick' | 'reject';
export type KeepRaw = 'auto' | 'keep' | 'drop';
export type ColorLabel = 'red' | 'yellow' | 'green' | 'blue' | 'purple';

export interface PhotoMeta {
  rating: number;
  flag: Flag;
  /** A ColorLabel, or free text another tool wrote. */
  label: string | null;
  keepRaw: KeepRaw;
}

export interface PhotoItem extends PhotoMeta {
  id: number;
  state: PhotoState;
  stem: string;
  /** Grid and editor path: the RAW if present. */
  representativePath: string | null;
  /** What the viewer shows. */
  displayPath: string | null;
  /** What the grid thumbnail is made from. */
  thumbnailPath: string | null;
  rawExt: string | null;
  displayExt: string | null;
  capturedAt: string | null;
  dateUncertain: boolean;
  guardMismatch: boolean;
  size: number;
  camera: string | null;
  lens: string | null;
}

export type Scope =
  | { kind: 'folder'; folderId: number; recursive: boolean }
  | { kind: 'dates'; from: string; to: string; locationId?: number };

export interface Sort {
  key?: 'time' | 'name' | 'rating' | 'size';
  descending?: boolean;
}

export interface Filter {
  minRating?: number;
  flags?: Flag[];
  labels?: string[];
  states?: PhotoState[];
  keepRaw?: KeepRaw[];
  camera?: string;
  lens?: string;
}

export interface DayCount {
  date: string;
  photos: number;
  uncertain: number;
}

export interface MemberFile {
  path: string;
  role: 'raw' | 'display' | 'extra' | 'sidecar';
  size: number;
  mtime: number;
}

export interface ExifSummary {
  dateOriginal: string | null;
  offsetOriginal: string | null;
  make: string | null;
  model: string | null;
  lens: string | null;
  width: number | null;
  height: number | null;
  iso: number | null;
  exposureS: number | null;
  fNumber: number | null;
  focalMm: number | null;
  gpsLat: number | null;
  gpsLon: number | null;
}

export interface PhotoDetail {
  photo: PhotoItem;
  files: MemberFile[];
  exif: ExifSummary | null;
}

/** 256 bins per channel, scaled 0..1 (RapidRAW's histogram). */
export interface Histogram {
  red: number[];
  green: number[];
  blue: number[];
  luma: number[];
}

/** Fields left out are not changed; `label: ''` clears the label. */
export interface MetaChange {
  rating?: number;
  flag?: Flag;
  label?: ColorLabel | '';
  keepRaw?: KeepRaw;
}

export interface SetResult {
  photoId: number;
  meta: PhotoMeta | null;
  error: string | null;
}

export interface FolderChanged {
  locationId: number;
  folders: number[];
  shoots: number[];
}

// ---- Commands ----

/** The delete dialog's choices (PRD §6.6). */
export type DeleteScope = 'photo' | 'keepDisplay' | 'keepRaw';

export interface DeleteOption {
  photos: number;
  files: number;
  bytes: number;
  /** Photos the Keep-RAW lock keeps out of Keep HIF only. */
  locked: number;
}

export interface DeletePreview {
  /** Photos with both a RAW and a display copy: the choices apply to these. */
  pairs: number;
  photo: DeleteOption;
  keepDisplay: DeleteOption;
  keepRaw: DeleteOption;
}

export interface DeleteResult {
  photos: number;
  files: number;
  bytes: number;
  locked: number;
}

/** Why files are in Recently Deleted (PRD §6.6). */
export type DeleteReason = 'deleted' | 'slimmed' | 'replaced';

export interface DeletedPhoto {
  /** Index in the operation's manifest (preview URL, restore). */
  index: number;
  stem: string;
  shoot: string;
  capturedAt: string | null;
  rating: number;
  files: string[];
  bytes: number;
}

export interface DeletedOp {
  opId: string;
  reason: DeleteReason;
  createdAt: number;
  expiresAt: number | null;
  bytes: number;
  photos: DeletedPhoto[];
  looseFiles: string[];
}

export interface DeletedLocation {
  locationId: number;
  name: string;
  status: LocationStatus;
  ops: DeletedOp[];
}

export interface Restored {
  files: number;
  /** Original paths taken again by a new file; those stay in Recently Deleted. */
  conflicts: string[];
}

export interface Undone {
  action: string;
  ops: number;
  files: number;
  conflicts: string[];
  locations: number[];
}

export type Verdict = 'keepRaw' | 'yourCall' | 'hifEnough';

export type RawValueChipKind =
  | 'rated'
  | 'picked'
  | 'edited'
  | 'highlightsClipped'
  | 'shadowsCrushed'
  | 'highContrast'
  | 'warmLight'
  | 'coolLight'
  | 'greenTint'
  | 'magentaTint'
  | 'highIso'
  | 'importantShoot'
  | 'rejected'
  | 'untouched'
  | 'casualShoot'
  | 'wellExposed';

export interface RawValueChip {
  kind: RawValueChipKind;
  toward: 'keep' | 'drop';
  value: number | null;
}

/** RAW-value recommendation (PRD §6.5). Recommends only. */
export interface RawValue {
  photoId: number;
  verdict: Verdict;
  /** Locked with K: rules never change it. */
  locked: boolean;
  /** Nobody rated, flagged or locked it. */
  unconfirmed: boolean;
  chips: RawValueChip[];
  noImage: boolean;
}

export interface Purged {
  ops: number;
  files: number;
  bytes: number;
}

/** Base of the `pvimg` scheme (Windows serves custom schemes over http). */
export const PVIMG_BASE = navigator.userAgent.includes('Windows') ? 'http://pvimg.localhost' : 'pvimg://localhost';

/** A small JPEG of a Photo in Recently Deleted. */
/** A Photo's preview (a RAW-only Photo gets its embedded preview). */
export const photoPreviewUrl = (photoId: number) => `${PVIMG_BASE}/photo/${photoId}?jpeg`;

export const deletedPreviewUrl = (locationId: number, opId: string, index: number) =>
  `${PVIMG_BASE}/deleted/${locationId}/${opId}/${index}`;

export const pvApi = {
  locations: {
    list: () => invoke<LocationInfo[]>('pv_locations_list'),
    add: (path: string, name?: string) => invoke<LocationInfo>('pv_locations_add', { path, name }),
    /** Forgets the location and its cache; files on disk are not touched. */
    remove: (id: number) => invoke<void>('pv_locations_remove', { id }),
    rename: (id: number, name: string) => invoke<void>('pv_locations_rename', { id, name }),
    reorder: (ids: number[]) => invoke<void>('pv_locations_reorder', { ids }),
    suggestRelocation: (id: number) => invoke<RelocationSuggestion[]>('pv_locations_suggest_relocation', { id }),
    relocate: (id: number, path: string) => invoke<LocationInfo>('pv_locations_relocate', { id, path }),
    rescan: (id: number) => invoke<void>('pv_locations_rescan', { id }),
    updateSettings: (id: number, settings: LocationSettings) =>
      invoke<LocationInfo>('pv_locations_update_settings', { id, settings }),
    /** Asks macOS to mount the location's network share. */
    connect: (id: number) => invoke<void>('pv_locations_connect', { id }),
  },
  folderTree: (locationId: number) => invoke<FolderNode[]>('pv_folder_tree', { locationId }),
  listPhotos: (scope: Scope, sort?: Sort, filter?: Filter) =>
    invoke<PhotoItem[]>('pv_list_photos', { scope, sort, filter }),
  dates: (locationId?: number) => invoke<DayCount[]>('pv_dates', { locationId }),
  photoDetail: (photoId: number) => invoke<PhotoDetail | null>('pv_photo_detail', { photoId }),
  histogram: (photoId: number) => invoke<Histogram>('pv_photo_histogram', { photoId }),
  /** Writes XMP + RapidRAW sidecars; each Photo succeeds or fails on its own. */
  setMetadata: (photoIds: number[], change: MetaChange) => invoke<SetResult[]>('pv_set_metadata', { photoIds, change }),
  /** What each delete choice would move to Recently Deleted. Changes nothing. */
  deletePreview: (photoIds: number[]) => invoke<DeletePreview>('pv_delete_preview', { photoIds }),
  /** Moves to Recently Deleted (30 days). `paths` are grid paths. */
  deletePhotos: (paths: string[], scope: DeleteScope) => invoke<DeleteResult>('pv_delete_photos', { paths, scope }),
  recentlyDeleted: () => invoke<DeletedLocation[]>('pv_recently_deleted'),
  /** Puts files back: the whole operation, or only some Photos (manifest indexes) / loose files. */
  restoreDeleted: (locationId: number, opId: string, photos?: number[], files?: string[]) =>
    invoke<Restored>('pv_restore_deleted', { locationId, opId, photos, files }),
  /** Empty now. Permanent: only files listed in Recently Deleted manifests are deleted. */
  emptyDeleted: (locationId?: number, opIds?: string[]) => invoke<Purged>('pv_empty_deleted', { locationId, opIds }),
  /** `⚠ RAW` Photos everywhere: RAWs whose display copy disappeared outside Pairview. */
  orphans: () => invoke<PhotoItem[]>('pv_orphans'),
  /** Orphan review "Keep as RAW-only": catalog state only, no files change. */
  keepRawOnly: (photoIds: number[]) => invoke<number>('pv_keep_raw_only', { photoIds }),
  /** Undoes the most recent delete still in Recently Deleted; null when there is none. */
  undo: () => invoke<Undone | null>('pv_undo'),
  /** Keep RAW / Your call / HIF is enough with reason chips. */
  rawValue: (photoIds: number[]) => invoke<RawValue[]>('pv_raw_value', { photoIds }),
};

/**
 * The Photos of a folder as upstream ImageFiles (one per Photo), or null when the folder isn't in
 * a scanned Pairview location (or the backend isn't ready): the caller then lists the disk.
 */
export async function pvImagesForPath(path: string | null, recursive: boolean): Promise<ImageFile[] | null> {
  const { setPhotos, clear } = usePvPhotoStore.getState();
  if (!path) {
    clear();
    return null;
  }
  try {
    const listing = await invoke<{ images: ImageFile[]; photos: PhotoItem[] } | null>('pv_list_images_for_path', {
      path,
      recursive,
    });
    if (!listing) {
      clear();
      return null;
    }
    setPhotos(listing.photos);
    return listing.images;
  } catch (e) {
    console.warn('Pairview listing failed, falling back to the folder listing:', e);
    clear();
    return null;
  }
}

export const FOLDER_CHANGED_EVENT = 'pv://folder-changed';

export function onFolderChanged(handler: (change: FolderChanged) => void): Promise<UnlistenFn> {
  return listen<FolderChanged>(FOLDER_CHANGED_EVENT, (event) => handler(event.payload));
}
