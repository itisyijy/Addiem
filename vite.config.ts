import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// tauri dev 는 고정 포트 1420 을 기다린다 (tauri.conf.json devUrl). clearScreen: false 는 Rust 오류를 가리지 않게
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true, watch: { ignored: ["**/src-tauri/**"] } },
});
