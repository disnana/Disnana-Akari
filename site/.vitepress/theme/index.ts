import DefaultTheme from 'vitepress/theme'
import type { Theme } from 'vitepress'
import { tokenize } from '../tokenize'
import './custom.css'

export default {
  extends: DefaultTheme,
  enhanceApp({ siteData }) {
    // 設定は JSON として配信されるので、関数（日本語の分割器）はブラウザー側で渡し直す。
    const search = (siteData.value.themeConfig as any)?.search
    if (search?.provider === 'local') {
      search.options ??= {}
      search.options.miniSearch = { options: { tokenize }, searchOptions: { tokenize } }
    }
  },
} satisfies Theme
