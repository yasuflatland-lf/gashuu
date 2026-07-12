import { asset } from '../lib/asset';

// Fixed translucent header: logo + centred nav jumping to in-page sections and
// the GitHub releases page.
export function Header() {
  return (
    <header className="fixed inset-x-0 top-0 z-50 border-b border-line bg-cream/90 backdrop-blur">
      <nav className="mx-auto grid max-w-[1000px] grid-cols-[1fr_auto_1fr] items-center px-6 py-4 md:px-10">
        {/* biome-ignore lint/a11y/useValidAnchor: logo link scrolls to the top of the page, no dedicated route exists */}
        <a href="#" className="flex items-center gap-3 justify-self-start">
          <img
            src={asset('assets/logo.svg')}
            alt="gashuu app icon"
            className="h-8 w-8 object-contain"
          />
          <span className="type-nav text-ink">gashuu</span>
        </a>
        <div className="type-nav hidden items-center gap-8 text-ink2 md:flex md:justify-self-center">
          <a href="#workflow" className="transition-colors hover:text-ink">
            機能
          </a>
          <a
            href="https://github.com/yasuflatland-lf/gashuu/releases"
            className="transition-colors hover:text-ink"
          >
            Download
          </a>
          <a href="#faq" className="transition-colors hover:text-ink">
            FAQ
          </a>
        </div>
        <div className="hidden md:block justify-self-end" aria-hidden="true" />
      </nav>
    </header>
  );
}
