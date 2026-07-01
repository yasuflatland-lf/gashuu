import type { ReactNode } from 'react';

// Small uppercase label — the page's only positively-tracked type and, by
// default, the quietest ink. No pill, no border: Aesop sets labels in plain
// tracked caps.
type KickerProps = {
  children: ReactNode;
  className?: string;
};

export function Kicker({ children, className = '' }: KickerProps) {
  return <p className={`type-kicker text-ink3 ${className}`}>{children}</p>;
}
