// Pairview: Settings → Library → Deleting (PRD §6.6). The same default the delete dialog's
// "Don't ask again" sets; "Ask every time" brings the dialog back.
import { useTranslation } from 'react-i18next';
import Dropdown from '../../components/ui/Dropdown';
import Text from '../../components/ui/Text';
import { TextColors, TextVariants } from '../../types/typography';
import { saveDeleteDefault, useDeleteDialog, type DeleteDefault } from './choose';

export default function DeleteSettings() {
  const { t } = useTranslation();
  const value = useDeleteDialog((s) => s.remembered);
  const options: { value: DeleteDefault; label: string }[] = [
    { value: 'ask', label: t('pv.delete.settings.ask') },
    { value: 'photo', label: t('pv.delete.scope.photo') },
    { value: 'keepDisplay', label: t('pv.delete.scope.keepDisplay', { display: 'HIF/JPG' }) },
    { value: 'keepRaw', label: t('pv.delete.scope.keepRaw') },
  ];
  return (
    <div className="p-6 bg-surface rounded-xl shadow-md">
      <Text variant={TextVariants.title} color={TextColors.accent} className="mb-5">
        {t('pv.delete.settings.title')}
      </Text>
      <div className="flex items-center justify-between gap-4">
        <div>
          <Text variant={TextVariants.heading}>{t('pv.delete.settings.label')}</Text>
          <Text className="mt-1 text-sm text-text-secondary">{t('pv.delete.settings.hint')}</Text>
        </div>
        <Dropdown className="w-56 shrink-0" value={value} options={options} onChange={(v) => saveDeleteDefault(v)} />
      </div>
    </div>
  );
}
