import { FaqSection } from './components/FaqSection';
import { FeaturesSection } from './components/FeaturesSection';
import { Footer } from './components/Footer';
import { Header } from './components/Header';
import { Hero } from './components/Hero';
import { PricingSection } from './components/PricingSection';
import { VideoSection } from './components/VideoSection';
import { asset } from './lib/asset';

export function App() {
  return (
    <div className="overflow-hidden">
      <Header />
      <Hero />
      <VideoSection
        kicker="Library"
        title="本棚を、流れるように。"
        lead="読み込んだ漫画は、そのままカバーフローへ。何百冊あっても、表紙を滑らせるように軽快に探せます。"
        src={asset('assets/carousel_demo.mp4')}
        poster={asset('assets/carousel_demo-poster.jpg')}
        alt="gashuuのライブラリで表紙のカバーフローをページ送りするデモ"
        ratio="946 / 634"
      />
      <FeaturesSection />
      <VideoSection
        kicker="Reader"
        title="高精細を、軽やかに。"
        lead="拡大してもくっきり精細。見開きもRTLも、なめらかなページ送りで読みに没頭できます。"
        src={asset('assets/view_demo.mp4')}
        poster={asset('assets/view_demo-poster.jpg')}
        alt="gashuuのビューアで高解像度の漫画をなめらかに読むデモ"
        ratio="900 / 634"
      />
      <PricingSection />
      <FaqSection />
      <Footer />
    </div>
  );
}
