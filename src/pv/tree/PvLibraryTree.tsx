// Pairview: Library section of the folder panel (PRD §6.1, docs/IMPLEMENTATION_PLAN.md M1-11).
// Shows each Library Location with its folders. A shoot's format sub-folders (arw/, hif/) are
// hidden: the shoot is one folder of Photos. Counts are Photos, not files.
import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import clsx from 'clsx';
import { ChevronDown, ChevronRight, Folder, FolderOpen, HardDrive, Server } from 'lucide-react';
import Text from '../../components/ui/Text';
import { TextColors, TextVariants, TextWeights } from '../../types/typography';
import { onFolderChanged, pvApi, type FolderNode, type LocationInfo } from '../api';
import PvDatesTree from './PvDatesTree';
import { RecentlyDeletedEntry } from '../deleted/RecentlyDeleted';
import { OrphanBanner } from '../orphans/OrphanReview';

interface PvLibraryTreeProps {
  onFolderSelect(path: string): void;
  selectedPath: string | null;
}

interface TreeItem {
  node: FolderNode;
  children: TreeItem[];
}

function buildTree(nodes: FolderNode[]): TreeItem | null {
  const items = new Map<number, TreeItem>();
  for (const node of nodes) if (!node.formatSubdir) items.set(node.id, { node, children: [] });
  let root: TreeItem | null = null;
  for (const item of items.values()) {
    const parent = item.node.parentId !== null ? items.get(item.node.parentId) : undefined;
    if (parent) parent.children.push(item);
    else if (item.node.parentId === null) root = item;
  }
  const sort = (item: TreeItem) => {
    item.children.sort((a, b) => a.node.name.localeCompare(b.node.name, undefined, { numeric: true }));
    item.children.forEach(sort);
  };
  if (root) sort(root);
  return root;
}

export default function PvLibraryTree({ onFolderSelect, selectedPath }: PvLibraryTreeProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(true);
  const [locations, setLocations] = useState<LocationInfo[]>([]);
  const [trees, setTrees] = useState<Record<number, TreeItem | null>>({});
  const [expanded, setExpanded] = useState<Set<number>>(new Set());

  const loadTree = useCallback(async (locationId: number) => {
    try {
      const tree = buildTree(await pvApi.folderTree(locationId));
      setTrees((prev) => ({ ...prev, [locationId]: tree }));
    } catch {
      // Not scanned yet, or the backend isn't ready: shown as empty.
    }
  }, []);

  const loadAll = useCallback(async () => {
    try {
      const list = await pvApi.locations.list();
      setLocations(list);
      await Promise.all(list.map((l) => loadTree(l.id)));
    } catch {
      setLocations([]);
    }
  }, [loadTree]);

  useEffect(() => {
    void loadAll();
    const unlisten = onFolderChanged((change) => void loadTree(change.locationId));
    return () => void unlisten.then((stop) => stop());
  }, [loadAll, loadTree]);

  // Locations added or removed in Settings show up when the panel regains focus.
  useEffect(() => {
    const onFocus = () => void loadAll();
    window.addEventListener('focus', onFocus);
    return () => window.removeEventListener('focus', onFocus);
  }, [loadAll]);

  const toggle = (id: number) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  if (locations.length === 0) return null;

  return (
    <>
      <OrphanBanner
        onChanged={() => {
          void loadAll();
          if (selectedPath) onFolderSelect(selectedPath);
        }}
      />
      <div className="pb-2">
        <div
          className="flex items-center w-full px-1 py-1.5 cursor-pointer rounded-md hover:bg-surface/30 transition-colors"
          onClick={() => setOpen((o) => !o)}
        >
          <Text as="div" variant={TextVariants.small} weight={TextWeights.bold} className="flex items-center min-w-0">
            <div className="p-0.5">{open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}</div>
            <span className="ml-1 uppercase tracking-wider select-none truncate">{t('pv.tree.title')}</span>
          </Text>
        </div>
        {open && (
          <div className="pt-1">
            {locations.map((location) => (
              <LocationRoot
                key={location.id}
                location={location}
                tree={trees[location.id] ?? null}
                expanded={expanded}
                onToggle={toggle}
                onFolderSelect={onFolderSelect}
                selectedPath={selectedPath}
              />
            ))}
          </div>
        )}
      </div>
      <PvDatesTree onSelect={onFolderSelect} selectedPath={selectedPath} />
      <RecentlyDeletedEntry onRestored={() => selectedPath && onFolderSelect(selectedPath)} />
    </>
  );
}

interface LocationRootProps {
  location: LocationInfo;
  tree: TreeItem | null;
  expanded: Set<number>;
  onToggle(id: number): void;
  onFolderSelect(path: string): void;
  selectedPath: string | null;
}

function LocationRoot({ location, tree, expanded, onToggle, onFolderSelect, selectedPath }: LocationRootProps) {
  const { t } = useTranslation();
  const online = location.status === 'online';
  const Icon = location.connectUrl ? Server : HardDrive;
  // Location roots start expanded; their ids are negative so they never clash with folders.
  const rootKey = -location.id;
  const isOpen = !expanded.has(rootKey);
  const isSelected = selectedPath === location.path;

  return (
    <div>
      <Row
        depth={0}
        selected={isSelected}
        dimmed={!online}
        icon={<Icon size={16} />}
        name={location.name}
        count={tree?.node.photosRecursive ?? location.photos}
        hasChildren={(tree?.children.length ?? 0) > 0}
        isOpen={isOpen}
        title={online ? location.path : `${location.path} · ${t(`pv.locations.status.${location.status}`)}`}
        onSelect={() => onFolderSelect(location.path)}
        onToggle={() => onToggle(rootKey)}
      />
      {isOpen &&
        tree?.children.map((child) => (
          <FolderRow
            key={child.node.id}
            item={child}
            depth={1}
            dimmed={!online}
            expanded={expanded}
            onToggle={onToggle}
            onFolderSelect={onFolderSelect}
            selectedPath={selectedPath}
          />
        ))}
    </div>
  );
}

interface FolderRowProps {
  item: TreeItem;
  depth: number;
  dimmed: boolean;
  expanded: Set<number>;
  onToggle(id: number): void;
  onFolderSelect(path: string): void;
  selectedPath: string | null;
}

function FolderRow({ item, depth, dimmed, expanded, onToggle, onFolderSelect, selectedPath }: FolderRowProps) {
  const isOpen = expanded.has(item.node.id);
  const hasChildren = item.children.length > 0;
  const subtitle = useMemo(
    () => (item.node.dateHint && !item.node.name.startsWith(item.node.dateHint) ? item.node.dateHint : null),
    [item.node.dateHint, item.node.name],
  );
  return (
    <>
      <Row
        depth={depth}
        selected={selectedPath === item.node.path}
        dimmed={dimmed}
        icon={isOpen ? <FolderOpen size={16} /> : <Folder size={16} />}
        name={item.node.name}
        subtitle={subtitle}
        count={item.node.photosRecursive}
        hasChildren={hasChildren}
        isOpen={isOpen}
        title={item.node.path}
        onSelect={() => onFolderSelect(item.node.path)}
        onToggle={() => onToggle(item.node.id)}
      />
      {isOpen &&
        item.children.map((child) => (
          <FolderRow
            key={child.node.id}
            item={child}
            depth={depth + 1}
            dimmed={dimmed}
            expanded={expanded}
            onToggle={onToggle}
            onFolderSelect={onFolderSelect}
            selectedPath={selectedPath}
          />
        ))}
    </>
  );
}

interface RowProps {
  depth: number;
  selected: boolean;
  dimmed: boolean;
  icon: React.ReactNode;
  name: string;
  /** Camera folder date (e.g. 2026-10-31 for 10561031) when the name doesn't already show it. */
  subtitle?: string | null;
  count: number;
  hasChildren: boolean;
  isOpen: boolean;
  title: string;
  onSelect(): void;
  onToggle(): void;
}

function Row({
  depth,
  selected,
  dimmed,
  icon,
  name,
  subtitle,
  count,
  hasChildren,
  isOpen,
  title,
  onSelect,
  onToggle,
}: RowProps) {
  return (
    <Text as="div" color={TextColors.primary} weight={TextWeights.medium}>
      <div
        title={title}
        style={{ paddingLeft: `${depth * 14 + 6}px` }}
        className={clsx('flex items-center gap-2 py-1.5 pr-1.5 rounded-md transition-colors cursor-pointer', {
          'bg-surface': selected,
          'hover:bg-card-active': !selected,
          'opacity-50': dimmed,
        })}
        onClick={onSelect}
      >
        <div
          className="w-5 h-5 flex items-center justify-center shrink-0 text-text-secondary"
          onClick={(e) => {
            if (!hasChildren) return;
            e.stopPropagation();
            onToggle();
          }}
        >
          {icon}
        </div>
        <span className="min-w-0 flex-1 select-none truncate">
          {name}
          {subtitle && <span className="ml-1.5 text-text-secondary text-xs">{subtitle}</span>}
        </span>
        {count > 0 && (
          <Text as="span" variant={TextVariants.small} color={TextColors.secondary} className="shrink-0 tabular-nums">
            {count}
          </Text>
        )}
        {hasChildren ? (
          <div
            className="p-0.5 rounded-sm hover:bg-surface/50 text-text-secondary"
            onClick={(e) => {
              e.stopPropagation();
              onToggle();
            }}
          >
            {isOpen ? <ChevronDown size={16} /> : <ChevronRight size={16} />}
          </div>
        ) : (
          <div className="w-5 h-5 shrink-0" aria-hidden="true" />
        )}
      </div>
    </Text>
  );
}
