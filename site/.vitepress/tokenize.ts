// 日本語の検索で単語を分けるための分割器。索引の作成（Node）と検索（ブラウザー）の両方で使う。
const segmenter = new Intl.Segmenter('ja-JP', { granularity: 'word' })

export const tokenize = (text: string): string[] =>
  Array.from(segmenter.segment(text))
    .filter((part) => part.isWordLike)
    .map((part) => part.segment.toLowerCase())
