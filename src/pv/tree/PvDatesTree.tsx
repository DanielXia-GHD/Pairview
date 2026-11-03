// Pairview: Year → Month → Day browsing with Photo counts and jump-to-date (M1-13).
import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import clsx from 'clsx';
import { CalendarDays, ChevronDown, ChevronRight, HelpCircle } from 'lucide-react';
import Text from '../../components/ui/Text';
import { TextColors, TextVariants, TextWeights } from '../../types/typography';
import { onFolderChanged, pvApi, type DayCount } from '../api';
import { datesPath, formatMonth, formatWeekday } from '../dates';

interface Month {
  key: string;
  days: DayCount[];
  photos: number;
}

interface Year {
  key: string;
  months: Month[];
  photos: number;
}

function group(days: DayCount[]): Year[] {
  const years = new Map<string, Map<string, DayCount[]>>();
  for (const day of days) {
    const [y, m] = day.date.split('-');
    const months = years.get(y) ?? new Map<string, DayCount[]>();
    const month = months.get(`${y}-${m}`) ?? [];
    month.push(day);
    months.set(`${y}-${m}`, month);
    years.set(y, months);
  }
  const sum = (list: DayCount[]) => list.reduce((n, d) => n + d.photos, 0);
  // Newest first, like a photo timeline.
  return [...years.entries()]
    .sort(([a], [b]) => b.localeCompare(a))
    .map(([key, months]) => {
      const monthList = [...months.entries()]
        .sort(([a], [b]) => b.localeCompare(a))
        .map(([mKey, mDays]) => ({
          key: mKey,
          days: [...mDays].sort((a, b) => b.date.localeCompare(a.date)),
          photos: sum(mDays),
        }));
      return { key, months: monthList, photos: monthList.reduce((n, m) => n + m.photos, 0) };
    });
}

interface PvDatesTreeProps {
  onSelect(path: string): void;
  selectedPath: string | null;
}

export default function PvDatesTree({ onSelect, selectedPath }: PvDatesTreeProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(true);
  const [days, setDays] = useState<DayCount[]>([]);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [jump, setJump] = useState('');

  const load = useCallback(async () => {
    try {
      setDays(await pvApi.dates());
    } catch {
      setDays([]);
    }
  }, []);

  useEffect(() => {
    void load();
    const unlisten = onFolderChanged(() => void load());
    const onFocus = () => void load();
    window.addEventListener('focus', onFocus);
    return () => {
      void unlisten.then((stop) => stop());
      window.removeEventListener('focus', onFocus);
    };
  }, [load]);

  const years = useMemo(() => group(days), [days]);

  // The newest year starts open.
  useEffect(() => {
    if (years.length > 0) setExpanded((prev) => (prev.size === 0 ? new Set([years[0].key]) : prev));
  }, [years]);

  const toggle = (key: string) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });

  /** Opens the chosen day, or the nearest later day with photos (else the nearest earlier one). */
  const jumpTo = (date: string) => {
    setJump(date);
    if (!date || days.length === 0) return;
    const sorted = [...days].sort((a, b) => a.date.localeCompare(b.date));
    const target = sorted.find((d) => d.date >= date) ?? sorted[sorted.length - 1];
    setExpanded((prev) => new Set([...prev, target.date.slice(0, 4), target.date.slice(0, 7)]));
    onSelect(datesPath(target.date));
  };

  if (days.length === 0) return null;

  return (
    <div className="pb-2">
      <div
        className="flex items-center w-full px-1 py-1.5 cursor-pointer rounded-md hover:bg-surface/30 transition-colors"
        onClick={() => setOpen((o) => !o)}
      >
        <Text as="div" variant={TextVariants.small} weight={TextWeights.bold} className="flex items-center min-w-0">
          <div className="p-0.5">{open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}</div>
          <span className="ml-1 uppercase tracking-wider select-none truncate">{t('pv.dates.title')}</span>
        </Text>
      </div>
      {open && (
        <div className="pt-1">
          <label className="flex items-center gap-2 px-1.5 pb-1.5" title={t('pv.dates.jump')}>
            <CalendarDays size={16} className="shrink-0 text-text-secondary" />
            <input
              type="date"
              value={jump}
              onChange={(e) => jumpTo(e.target.value)}
              onKeyDown={(e) => e.stopPropagation()}
              aria-label={t('pv.dates.jump')}
              className="min-w-0 flex-1 rounded-md bg-surface px-2 py-1 text-sm text-text-primary"
            />
          </label>
          {years.map((year) => (
            <div key={year.key}>
              <DateRow
                depth={0}
                label={year.key}
                count={year.photos}
                isOpen={expanded.has(year.key)}
                selected={selectedPath === datesPath(year.key)}
                onSelect={() => onSelect(datesPath(year.key))}
                onToggle={() => toggle(year.key)}
              />
              {expanded.has(year.key) &&
                year.months.map((month) => (
                  <div key={month.key}>
                    <DateRow
                      depth={1}
                      label={formatMonth(month.key)}
                      count={month.photos}
                      isOpen={expanded.has(month.key)}
                      selected={selectedPath === datesPath(month.key)}
                      onSelect={() => onSelect(datesPath(month.key))}
                      onToggle={() => toggle(month.key)}
                    />
                    {expanded.has(month.key) &&
                      month.days.map((day) => (
                        <DateRow
                          key={day.date}
                          depth={2}
                          label={`${Number(day.date.slice(8))} ${formatWeekday(day.date)}`}
                          count={day.photos}
                          uncertain={day.uncertain > 0 ? t('pv.dates.uncertain', { count: day.uncertain }) : undefined}
                          selected={selectedPath === datesPath(day.date)}
                          onSelect={() => onSelect(datesPath(day.date))}
                        />
                      ))}
                  </div>
                ))}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

interface DateRowProps {
  depth: number;
  label: string;
  count: number;
  selected: boolean;
  isOpen?: boolean;
  uncertain?: string;
  onSelect(): void;
  onToggle?(): void;
}

function DateRow({ depth, label, count, selected, isOpen, uncertain, onSelect, onToggle }: DateRowProps) {
  return (
    <Text as="div" color={TextColors.primary} weight={TextWeights.medium}>
      <div
        style={{ paddingLeft: `${depth * 14 + 6}px` }}
        className={clsx('flex items-center gap-2 py-1.5 pr-1.5 rounded-md transition-colors cursor-pointer', {
          'bg-surface': selected,
          'hover:bg-card-active': !selected,
        })}
        onClick={onSelect}
      >
        <span className="min-w-0 flex-1 select-none truncate">{label}</span>
        {uncertain && (
          <span data-tooltip={uncertain} className="shrink-0 text-yellow-400">
            <HelpCircle size={13} />
          </span>
        )}
        <Text as="span" variant={TextVariants.small} color={TextColors.secondary} className="shrink-0 tabular-nums">
          {count}
        </Text>
        {onToggle ? (
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
