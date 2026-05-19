import { invoke } from '@tauri-apps/api/core';

export interface Health {
  version: string;
  db_ready: boolean;
  schema_version: number;
}

export async function healthCheck(): Promise<Health> {
  return invoke<Health>('health_check');
}
