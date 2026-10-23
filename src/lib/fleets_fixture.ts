// Test fixture (imported only by tests): a `FleetInfo` row. The primary is
// this machine's own fleet (no url, not connected to a hub); any other id is
// a connected extra fleet. Each test passes only what it needs different.
import { PRIMARY, type FleetInfo } from './fleets';

export function fleetInfo(id: string, over: Partial<FleetInfo> = {}): FleetInfo {
  const primary = id === PRIMARY;
  return {
    id,
    label: id,
    url: primary ? null : `https://${id}.example.com`,
    color: '#5b8def',
    primary,
    local: primary,
    client_name: null,
    connection: primary ? null : { state: 'connected' },
    unavailable: null,
    ...over,
  };
}
