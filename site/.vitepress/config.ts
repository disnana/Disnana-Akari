import { defineConfig } from 'vitepress'
import { tokenize } from './tokenize'

const repo = 'https://github.com/disnana/Disnana-Akari'

// 見出しの ID。標準では濁点・半濁点が分解されて、`#ボタン` のようなリンクが一致しなくなるので、NFC に正規化する。
// それ以外の規則（記号を `-` にする、数字で始まるときは `_` を付ける）は標準と同じ。
const slugify = (text: string): string =>
  text
    .normalize('NFC')
    .replace(/[\u0000-\u001f]/g, '')
    .replace(/[\s~`!@#$%^&*()\-_+=[\]{}|\\;:"'“”‘’–—<>,.?/]+/g, '-')
    .replace(/-{2,}/g, '-')
    .replace(/^-+|-+$/g, '')
    .replace(/^(\d)/, '_$1')
    .toLowerCase()

export default defineConfig({
  title: 'Disnana Akari',
  description: 'Rust 製 Discord Bot フレームワーク Disnana Akari のドキュメント',
  lang: 'ja-JP',
  base: '/Disnana-Akari/',
  srcDir: 'content',
  cleanUrls: true,
  head: [
    ['link', { rel: 'icon', type: 'image/svg+xml', href: '/Disnana-Akari/favicon.svg' }],
    ['meta', { property: 'og:type', content: 'website' }],
    ['meta', { property: 'og:site_name', content: 'Disnana Akari' }],
  ],
  markdown: {
    anchor: { slugify },
    theme: { light: 'github-light', dark: 'github-dark' },
  },
  themeConfig: {
    logo: '/favicon.svg',
    nav: [
      { text: 'ガイド', link: '/getting-started/install', activeMatch: '^/(getting-started|guide)/' },
      { text: 'リファレンス', link: '/reference/faq', activeMatch: '^/reference/' },
      { text: 'API', link: '/api/disnana_akari/index.html', target: '_blank', rel: 'noopener' },
    ],
    sidebar: [
      { text: 'はじめに', items: [{ text: 'Akari とは', link: '/introduction' }] },
      {
        text: 'はじめよう',
        items: [
          { text: 'インストール', link: '/getting-started/install' },
          { text: 'Discord で Bot を用意する', link: '/getting-started/discord-setup' },
          { text: 'はじめての Bot', link: '/getting-started/first-bot' },
          { text: 'Rust が初めての人へ', link: '/getting-started/rust-basics' },
          { text: 'discord.py との対応表', link: '/getting-started/from-discordpy' },
        ],
      },
      {
        text: 'ガイド',
        items: [
          { text: 'Intent の設定', link: '/guide/intents' },
          { text: 'テキストコマンド', link: '/guide/text-commands' },
          { text: 'スラッシュコマンド', link: '/guide/slash-commands' },
          { text: 'ハイブリッドコマンド', link: '/guide/hybrid-commands' },
          { text: '権限の確認', link: '/guide/permissions' },
          { text: 'モジュールで整理する', link: '/guide/modules' },
          { text: '定期処理', link: '/guide/tasks' },
          { text: 'ステータスとアクティビティ', link: '/guide/presence' },
          { text: 'イベントを受け取る', link: '/guide/events' },
          { text: '応答のしかた', link: '/guide/responding' },
          { text: 'エラーと自動で送る文言', link: '/guide/errors' },
          { text: 'ログを出す', link: '/guide/logging' },
          { text: 'Embed とボタン', link: '/guide/buttons' },
          { text: '選択メニュー', link: '/guide/select-menus' },
          { text: '入力フォーム（モーダル）', link: '/guide/modals' },
          { text: 'Components V2', link: '/guide/components-v2' },
          { text: 'クールダウン', link: '/guide/cooldowns' },
          { text: 'Akari の動き方', link: '/guide/how-it-works' },
          { text: 'Twilight を直接使う', link: '/guide/twilight' },
          { text: 'Discord の REST 操作', link: '/guide/discord-rest' },
        ],
      },
      {
        text: 'リファレンス',
        items: [
          { text: 'API リファレンス', link: '/reference/api' },
          { text: 'よくある質問', link: '/reference/faq' },
          { text: '用語集', link: '/reference/glossary' },
          { text: '変更履歴', link: '/reference/changelog' },
          { text: '0.1 から 0.2 への移行', link: '/reference/migrating-0.2' },
        ],
      },
    ],
    socialLinks: [{ icon: 'github', link: repo }],
    editLink: {
      pattern: `${repo}/edit/main/site/content/:path`,
      text: 'GitHub でこのページを編集',
    },
    footer: { message: 'MIT License', copyright: 'Copyright © Disnana' },
    outline: { level: [2, 3], label: 'このページの内容' },
    docFooter: { prev: '前のページ', next: '次のページ' },
    returnToTopLabel: '先頭へ戻る',
    sidebarMenuLabel: 'メニュー',
    darkModeSwitchLabel: '外観',
    lightModeSwitchTitle: 'ライトテーマに切り替え',
    darkModeSwitchTitle: 'ダークテーマに切り替え',
    langMenuLabel: '言語',
    search: {
      provider: 'local',
      options: {
        miniSearch: {
          options: { tokenize },
          searchOptions: { tokenize },
        },
        translations: {
          button: { buttonText: '検索', buttonAriaLabel: '検索' },
          modal: {
            displayDetails: '詳細を表示',
            resetButtonTitle: '入力を消す',
            backButtonTitle: '戻る',
            noResultsText: '見つかりませんでした',
            footer: { selectText: '選択', selectKeyAriaLabel: 'Enter', navigateText: '移動', navigateUpKeyAriaLabel: '上矢印', navigateDownKeyAriaLabel: '下矢印', closeText: '閉じる', closeKeyAriaLabel: 'Esc' },
          },
        },
      },
    },
  },
})
