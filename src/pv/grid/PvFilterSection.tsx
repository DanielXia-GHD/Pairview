// Pairview: Photo-type and Keep-RAW filters inside the library's filter menu.
import { useTranslation } from 'react-i18next';
import Text from '../../components/ui/Text';
import { TextVariants, TextWeights } from '../../types/typography';
import { PV_AI_FILTERS, PV_KEEP_RAW_FILTERS, PV_PHOTO_FILTERS, type PvFilter } from './filter';

interface SwitchProps {
  options: Array<{ id: string; label: string }>;
  value: string;
  onChange(value: string): void;
}

interface PvFilterSectionProps {
  filter?: PvFilter;
  onChange(filter: PvFilter): void;
  /** The menu's own segmented switch, so the section looks like the rest of the menu. */
  Switch: React.ComponentType<SwitchProps>;
}

export default function PvFilterSection({ filter, onChange, Switch }: PvFilterSectionProps) {
  const { t } = useTranslation();
  return (
    <>
      <div>
        <Text as="div" variant={TextVariants.small} weight={TextWeights.semibold} className="px-3 py-1 uppercase">
          {t('pv.grid.filter.photoTitle')}
        </Text>
        <div className="px-3 mt-1">
          <Switch
            options={PV_PHOTO_FILTERS.map((id) => ({ id, label: t(`pv.grid.filter.photo.${id}`) }))}
            value={filter?.photo ?? 'all'}
            onChange={(photo) => onChange({ ...filter, photo: photo as PvFilter['photo'] })}
          />
        </div>
      </div>
      <div>
        <Text as="div" variant={TextVariants.small} weight={TextWeights.semibold} className="px-3 py-1 uppercase">
          {t('pv.grid.filter.keepRawTitle')}
        </Text>
        <div className="px-3 mt-1">
          <Switch
            options={PV_KEEP_RAW_FILTERS.map((id) => ({ id, label: t(`pv.grid.filter.keepRaw.${id}`) }))}
            value={filter?.keepRaw ?? 'all'}
            onChange={(keepRaw) => onChange({ ...filter, keepRaw: keepRaw as PvFilter['keepRaw'] })}
          />
        </div>
      </div>
      <div>
        <Text as="div" variant={TextVariants.small} weight={TextWeights.semibold} className="px-3 py-1 uppercase">
          {t('pv.grid.filter.aiTitle')}
        </Text>
        <div className="px-3 mt-1">
          <Switch
            options={PV_AI_FILTERS.map((id) => ({ id, label: t(`pv.grid.filter.ai.${id}`) }))}
            value={filter?.ai ?? 'all'}
            onChange={(ai) => onChange({ ...filter, ai: ai as PvFilter['ai'] })}
          />
        </div>
      </div>
    </>
  );
}
