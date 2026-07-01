import { asset } from '../lib/asset';

// Inverted (ink) footer: logo + license/copyright line.
export function Footer() {
  return (
    <footer className="bg-ink text-cream">
      <div className="mx-auto flex max-w-[1000px] flex-col justify-between gap-5 px-6 py-12 md:flex-row md:items-center md:px-10">
        <div className="flex items-center gap-3">
          <img
            src={asset('assets/logo.png')}
            alt="gashuu app icon"
            className="h-7 w-7 object-contain"
          />
          <span className="type-nav">gashuu</span>
        </div>
        <div className="type-body text-cream/55">
          Open source · MIT License · Copyright © 2026 Yasuyuki Takeo
        </div>
      </div>
    </footer>
  );
}
