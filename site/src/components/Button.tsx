import type { ReactNode } from 'react';

// Square, shadowless buttons. Primary is sumi-filled; secondary is a hairline
// outline. The seal red is deliberately NOT used here — buttons stay ink so the
// red reads as a signature, not an action color.
type ButtonProps = {
  children: ReactNode;
  variant?: 'primary' | 'secondary';
  href?: string;
};

export function Button({ children, variant = 'primary', href = '#download' }: ButtonProps) {
  const base = 'type-nav inline-flex items-center justify-center px-6 py-3 transition-colors';
  const styles =
    variant === 'primary'
      ? 'bg-ink text-cream border border-ink hover:bg-[#1f1e1c]'
      : 'bg-transparent text-ink border border-ink/25 hover:border-ink';
  return (
    <a href={href} className={`${base} ${styles}`}>
      {children}
    </a>
  );
}
