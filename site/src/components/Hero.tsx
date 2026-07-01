import { Button } from './Button';
import { Kicker } from './Kicker';

// Above-the-fold hero: kicker, serif headline, lead, primary/secondary CTAs, and
// a quiet open-source/license line.
export function Hero() {
  return (
    <section className="px-6 pt-[6rem] pb-[4rem] md:px-10 md:pt-[9rem] md:pb-[6rem]">
      <div className="mx-auto max-w-[1000px] text-center">
        <Kicker>Desktop manga reader</Kicker>
        <h1 className="type-hero rhythm-24 text-balance">
          漫画を、
          <wbr />
          美しく読む。
        </h1>
        <p className="type-lead mx-auto rhythm-32 measure text-pretty text-ink2">
          gashuuは、高解像度コミックを軽快に読むためのデスクトップビューア。フォルダもアーカイブも、見開きもRTLも、作品ごとの読み心地も記憶。
        </p>
        <div className="rhythm-40 flex flex-col justify-center gap-3 sm:flex-row">
          <Button href="https://github.com/yasuflatland-lf/gashuu/releases">Get gashuu</Button>
          <Button variant="secondary" href="https://github.com/yasuflatland-lf/gashuu">
            View on GitHub →
          </Button>
        </div>
        <p className="type-nav rhythm-32 font-normal text-ink3">
          Open source · MIT License · 画集 / gashuu
        </p>
      </div>
    </section>
  );
}
