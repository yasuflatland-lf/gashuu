import { Kicker } from './Kicker';

// "Reading flow" — three feature columns (import / read / return), divided by
// hairlines. Anchored at #workflow for the header nav.
const features: [string, string, string][] = [
  ['IMPORT', 'どんな漫画も', 'フォルダもCBZ/ZIP/CBR/RARも自動判定。中身を見て適切に読み込みます。'],
  [
    'READ',
    '気持ちよく読む',
    '単ページ/見開き/Auto、RTL/LTR、フィットを作品ごとに記憶。キャッシュとプリロードでテンポよくめくれます。',
  ],
  [
    'RETURN',
    '続きへ戻る',
    '進捗、最近読んだ本、最後のページを保存。サムネイルやスクラブで探したい場面にもすぐ戻れます。',
  ],
];

export function FeaturesSection() {
  return (
    <section className="px-6 py-[4rem] md:px-10 md:py-[7.5rem]" id="workflow">
      <div className="mx-auto max-w-[1000px]">
        <div className="mx-auto measure text-center">
          <Kicker>Reading flow</Kicker>
          <h2 className="type-section rhythm-24 text-balance">開いて、整えて、続きへ。</h2>
          <p className="type-lead rhythm-24 text-pretty text-ink2">
            フォルダもアーカイブも自然に本棚へ。作品ごとの設定を記憶するので、次に開けば昨日のページからすぐ戻れます。
          </p>
        </div>
        <div className="rhythm-64 grid sm:grid-cols-3">
          {features.map(([kicker, title, body], i) => (
            <div
              key={title}
              className={`px-2 text-center sm:px-8 ${i > 0 ? 'mt-10 border-t border-line pt-10 sm:mt-0 sm:border-l sm:border-t-0 sm:pt-0' : ''}`}
            >
              <Kicker>{kicker}</Kicker>
              <h3 className="type-card-title rhythm-24">{title}</h3>
              <p className="type-body rhythm-12 text-ink2">{body}</p>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}
