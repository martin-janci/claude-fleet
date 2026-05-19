import '@testing-library/jest-dom/vitest';
import { vi } from 'vitest';
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => ({ version: '0.0.0', db_ready: true })),
}));
