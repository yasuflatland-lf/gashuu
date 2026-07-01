import { Kicker } from './Kicker';

// FAQ accordion built on native <details>. Anchored at #faq for the header nav.
const faqs: [string, string][] = [
  [
    'どんな形式に対応していますか？',
    '画像フォルダ、PNG/JPG/JPEG/AVIF、CBZ/ZIP/CBR/RARアーカイブを想定しています。',
  ],
  [
    '漫画の右綴じで読めますか？',
    'はい。RTL/LTRを切り替えられ、矢印キーやスクラブの挙動も読み方向に合わせます。',
  ],
  [
    '設定は作品ごとに残りますか？',
    '読書方向、見開き、カバー配置、フィットモードは本ごとに保存できます。',
  ],
  [
    '重い画像でも安全ですか？',
    '巨大画像や展開爆弾を事前にチェックし、問題があればクラッシュではなくステータスで通知する設計です。',
  ],
];

export function FaqSection() {
  return (
    <section className="mx-auto max-w-[760px] px-6 py-[4rem] md:px-10 md:py-[6rem]" id="faq">
      <Kicker className="text-center">Support</Kicker>
      <h2 className="type-section rhythm-24 text-center text-balance">FAQ</h2>
      <div className="rhythm-40 border-t border-line">
        {faqs.map(([q, a]) => (
          <details key={q} className="group border-b border-line py-5">
            <summary className="type-nav flex cursor-pointer list-none items-center justify-between text-ink">
              {q}
              <span className="ml-4 text-ink3 transition-transform group-open:rotate-45">+</span>
            </summary>
            <p className="type-body rhythm-12 text-ink2">{a}</p>
          </details>
        ))}
      </div>
    </section>
  );
}
