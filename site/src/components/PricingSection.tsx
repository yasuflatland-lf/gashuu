import { Kicker } from './Kicker';

// Pricing/CTA card: free, MIT, with a feature list and a download button.
// Anchored at #pricing; the download link carries id="download" (Button's
// default href target).
export function PricingSection() {
  return (
    <section className="px-6 py-[4rem] md:px-10 md:py-[6rem]" id="pricing">
      <div className="mx-auto max-w-[1000px] border border-line">
        <div className="grid lg:grid-cols-[1fr_360px]">
          <div className="p-8 md:p-14">
            <Kicker>Open source desktop app</Kicker>
            <h2 className="type-section rhythm-24 text-balance">無料で使える。</h2>
            <p className="type-lead rhythm-24 measure text-pretty text-ink2">
              gashuuはMITライセンスのオープンソース。まずは使って、要望はGitHubのIssueやPRで。
            </p>
          </div>
          <div className="border-t border-line bg-cream-alt p-8 md:p-10 lg:border-l lg:border-t-0">
            <h3 className="type-card-title">gashuu Desktop</h3>
            <div className="rhythm-12 font-serif text-[clamp(2.5rem,2rem+2vw,3.25rem)] font-medium leading-none">
              ¥0
            </div>
            <ul className="rhythm-32 type-body space-y-3 text-ink2">
              <li className="flex gap-3">
                <span className="text-ink3" aria-hidden="true">
                  —
                </span>
                Mac / Windows / Linux
              </li>
              <li className="flex gap-3">
                <span className="text-ink3" aria-hidden="true">
                  —
                </span>
                フォルダ &amp; コミックアーカイブ
              </li>
              <li className="flex gap-3">
                <span className="text-ink3" aria-hidden="true">
                  —
                </span>
                見開き / RTL / ズーム / サムネイル
              </li>
              <li className="flex gap-3">
                <span className="text-ink3" aria-hidden="true">
                  —
                </span>
                日本語 / 英語UI
              </li>
            </ul>
            <a
              id="download"
              href="https://github.com/yasuflatland-lf/gashuu/releases"
              className="rhythm-40 type-nav inline-flex w-full justify-center border border-ink bg-ink px-6 py-3 text-cream transition-colors hover:bg-[#1f1e1c]"
            >
              Download release
            </a>
          </div>
        </div>
      </div>
    </section>
  );
}
