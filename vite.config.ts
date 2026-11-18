import tailwindcss from '@tailwindcss/vite';
import react from '@vitejs/plugin-react';
import path from 'path';
import {defineConfig, type Plugin} from 'vite';

function fileSchemeCompatPlugin(): Plugin {
  return {
    name: 'miku-file-scheme-compat',
    apply: 'build',
    enforce: 'post',
    transformIndexHtml(html) {
      return html
        .replace(/<script\s+type="module"\s+crossorigin\s+src="([^"]+)"\s*><\/script>/gi, '<script defer src="$1"></script>')
        .replace(/<link\s+rel="stylesheet"\s+crossorigin\s+href="([^"]+)"\s*>/gi, '<link rel="stylesheet" href="$1">');
    },
  };
}

export default defineConfig(() => {
  return {
    base: './',
    plugins: [react(), tailwindcss(), fileSchemeCompatPlugin()],
    build: {
      rollupOptions: {
        output: {
          format: 'iife' as const,
        },
      },
    },
    resolve: {
      alias: {
        '@': path.resolve(__dirname, '.'),
      },
    },
    server: {
      // HMR is disabled in AI Studio via DISABLE_HMR env var.
      // Do not modify—file watching is disabled to prevent flickering during agent edits.
      hmr: process.env.DISABLE_HMR !== 'true',
      // Disable file watching when DISABLE_HMR is true to save CPU during agent edits.
      watch: process.env.DISABLE_HMR === 'true' ? null : {},
    },
  };
});
