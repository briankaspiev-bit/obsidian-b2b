import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { viteSingleFile } from 'vite-plugin-singlefile';

// `--mode singlefile` inlines everything into one HTML file for sharing a
// clickable preview. The Tauri shell will use the normal multi-file build.
export default defineConfig(({ mode }) => ({
  plugins: [react(), ...(mode === 'singlefile' ? [viteSingleFile()] : [])],
  build: { outDir: mode === 'singlefile' ? 'dist-preview' : 'dist' },
}));
