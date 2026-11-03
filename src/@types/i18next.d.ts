import 'i18next';
import en from '../i18n/locales/en.json';
import pvEn from '../pv/i18n/en.json'; // PV: Pairview strings

declare module 'i18next' {
  interface CustomTypeOptions {
    defaultNS: 'translation';
    resources: {
      translation: typeof en & typeof pvEn; // PV: Pairview strings
    };
  }
}
