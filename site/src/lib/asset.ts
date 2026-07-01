// Resolve a path under public/ against Vite's configured base (`/gashuu/` in
// production, `/` in dev) so assets load correctly on the GitHub Pages subpath.
export function asset(path: string): string {
  return `${import.meta.env.BASE_URL}${path}`;
}
