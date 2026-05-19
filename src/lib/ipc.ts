import { invoke } from '@tauri-apps/api/core';

export interface Health {
  version: string;
  db_ready: boolean;
}

export async function healthCheck(): Promise<Health> {
  return invoke<Health>('health_check');
}
