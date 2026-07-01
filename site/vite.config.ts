import tailwindcss from '@tailwindcss/vite';
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

// base: served from the GitHub Pages project subpath (yasuflatland-lf.github.io/gashuu).
// Public assets are resolved against import.meta.env.BASE_URL (see src/lib/asset.ts).
export default defineConfig({
  base: '/gashuu/',
  plugins: [react(), tailwindcss()],
});
