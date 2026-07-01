import { useEffect, useRef } from 'react';
import { Kicker } from './Kicker';

// A centred video showcase: heading + lead above a framed, autoplaying loop.
// The clip plays muted with no chrome (apple.com-style); it starts only when
// scrolled into view and pauses when out, and falls back to a poster + manual
// controls under prefers-reduced-motion. The frame is square with a single
// hairline border — no rounding, no shadow.
type VideoSectionProps = {
  kicker: string;
  title: string;
  lead: string;
  src: string;
  poster: string;
  alt: string;
  ratio: string;
};

export function VideoSection({ kicker, title, lead, src, poster, alt, ratio }: VideoSectionProps) {
  const videoRef = useRef<HTMLVideoElement>(null);
  useEffect(() => {
    const v = videoRef.current;
    if (!v) {
      return;
    }
    // `muted` is set imperatively: a muted JSX prop does not reliably set the
    // DOM property, and muting is required for autoplay to be permitted.
    v.muted = true;
    if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
      // Honor reduced motion: no autoplay; expose controls over the poster.
      v.removeAttribute('autoplay');
      v.setAttribute('controls', '');
      v.pause();
      return;
    }
    // Play only while on screen; pause when scrolled away.
    const io = new IntersectionObserver(
      (entries) => {
        for (const e of entries) {
          if (e.isIntersecting) {
            v.play().catch(() => {
              // Autoplay may be blocked before a user gesture; the poster remains.
            });
          } else {
            v.pause();
          }
        }
      },
      { threshold: 0.25 },
    );
    io.observe(v);
    return () => io.disconnect();
  }, []);
  return (
    <section className="mx-auto max-w-[1000px] px-6 py-[4rem] md:px-10 md:py-[6rem]">
      <div className="mx-auto measure text-center">
        <Kicker>{kicker}</Kicker>
        <h2 className="type-section rhythm-24 text-balance">{title}</h2>
        <p className="type-lead rhythm-24 text-pretty text-ink2">{lead}</p>
      </div>
      <div className="rhythm-40 mx-auto w-full max-w-4xl">
        <div
          className="relative overflow-hidden border border-line bg-black"
          style={{ aspectRatio: ratio }}
        >
          <video
            ref={videoRef}
            autoPlay
            muted
            loop
            playsInline
            preload="metadata"
            poster={poster}
            aria-label={alt}
            className="absolute inset-0 h-full w-full object-cover"
          >
            <source src={src} type="video/mp4" />
          </video>
        </div>
      </div>
    </section>
  );
}
